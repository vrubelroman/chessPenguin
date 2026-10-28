//! Whole-game analysis on the desktop: the background engine job over the main line,
//! the evaluation graph under the board, the report tab and the launch dialog. The
//! evaluations, judgements, cache and the job itself are the `game-review` crate's,
//! shared with the Android app.
//!
//! The job uses its own engine process and never touches the game; writing the results
//! into the game is a separate, undoable action ([`Review::write_to_game`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use chess_core::shakmaty::{Color, Move};
use chess_core::{Game, Nag, NodeId};
use egui::{Color32, RichText, Sense, Stroke, Ui, pos2, vec2};
use game_review::{Eval, GameAnalysis, JobEvent, JobFailure, JobSettings, Judgement, ReviewMeta};

pub use game_review::Mark;

use crate::i18n::{tr, trf};
use crate::settings::{EngineConfig, REVIEW_QUALITY, ReviewSettings, Settings};

pub enum ReportAction {
    Goto(NodeId),
    Write,
    Update,
    Reanalyse,
}

pub enum DialogAction {
    Start,
    EngineSwitch,
    OpenEngines,
}

enum JobMsg {
    Event(JobEvent),
    Done,
    Failed(String),
}

struct Job {
    rx: Receiver<JobMsg>,
    cancel: Arc<AtomicBool>,
    total: usize,
    done: usize,
    threads: u32,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct Review {
    /// Evaluations of the main line and the report built from them.
    pub analysis: GameAnalysis,
    job: Option<Job>,
    pub error: Option<String>,
    /// Graph and report tab are shown.
    pub visible: bool,
    pub dialog_open: bool,
    /// Measured engine speed per thread, for time estimates.
    nps_per_thread: Option<f64>,
    /// Report tab: index of the last visited error per side and kind.
    cycle: HashMap<(Color, Judgement), usize>,
}

/// The folder of the cached analyses.
fn cache_dir() -> Option<PathBuf> {
    Settings::cache_dir().map(|d| d.join("analysis"))
}

impl Review {
    pub fn new(game: &Game) -> Self {
        let analysis = GameAnalysis::new(game, cache_dir().as_deref());
        Review {
            visible: analysis.has_data(),
            analysis,
            job: None,
            error: None,
            dialog_open: false,
            nps_per_thread: None,
            cycle: HashMap::new(),
        }
    }

    pub fn has_data(&self) -> bool {
        self.analysis.has_data()
    }

    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    pub fn progress(&self) -> Option<(usize, usize)> {
        self.job.as_ref().map(|j| (j.done, j.total))
    }

    /// Positions of the current main line without an evaluation.
    pub fn missing(&self) -> usize {
        self.analysis.missing()
    }

    /// Re-derives the report when the game or the evaluations changed. Cheap otherwise.
    pub fn refresh(&mut self, game: &Game) {
        self.analysis.refresh(game);
    }

    /// Starts the engine job. `full` discards earlier results; otherwise only positions
    /// without an evaluation are analysed. Returns false if there is nothing to do.
    pub fn start(
        &mut self,
        game: &Game,
        engine: &EngineConfig,
        rs: &ReviewSettings,
        full: bool,
        ctx: &egui::Context,
    ) -> bool {
        self.job = None;
        self.error = None;
        let Some(path) = engine.executable() else {
            self.error = Some(tr("The built-in engine is not part of this build").into());
            return false;
        };
        let meta = ReviewMeta {
            engine: engine.name.clone(),
            nodes: rs.nodes(),
        };
        let targets = self.analysis.begin(game, meta, full);
        if targets.is_empty() {
            return false;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let total = targets.len();
        // Filling in keeps the quality of the earlier run (see `GameAnalysis::begin`).
        let nodes = self.analysis.meta.as_ref().map_or(rs.nodes(), |m| m.nodes);
        let settings = JobSettings {
            threads: rs.threads.max(1),
            hash_mb: rs.hash_mb,
            nodes,
            eval_file: None,
        };
        let threads = settings.threads;
        let (cancel_w, ctx_w) = (cancel.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("game-review".into())
            .spawn(move || {
                let result = game_review::run(
                    uci::EngineConfig::new(&path),
                    &settings,
                    targets,
                    &cancel_w,
                    &mut |e| {
                        let _ = tx.send(JobMsg::Event(e));
                        ctx_w.request_repaint();
                    },
                );
                let msg = match result {
                    Ok(()) => JobMsg::Done,
                    Err(JobFailure::Start(e) | JobFailure::Engine(e)) => JobMsg::Failed(e),
                    Err(JobFailure::Terminated(reason)) => {
                        JobMsg::Failed(crate::analysis::termination_message(&reason))
                    }
                };
                let _ = tx.send(msg);
                ctx_w.request_repaint();
            });
        if let Err(e) = spawned {
            self.error = Some(e.to_string());
            return false;
        }
        self.job = Some(Job {
            rx,
            cancel,
            total,
            done: 0,
            threads,
        });
        self.visible = true;
        true
    }

    pub fn cancel(&mut self) {
        if self.job.take().is_some() {
            self.save_cache();
        }
    }

    /// Collects job results; returns true when the job has just ended.
    pub fn poll(&mut self) -> bool {
        let Some(job) = &mut self.job else {
            return false;
        };
        let mut ended = false;
        while let Ok(msg) = job.rx.try_recv() {
            match msg {
                JobMsg::Event(e) => {
                    if let JobEvent::Eval { nps, .. } = &e {
                        job.done += 1;
                        if *nps > 0 {
                            self.nps_per_thread = Some(*nps as f64 / f64::from(job.threads));
                        }
                    }
                    self.analysis.apply(e);
                }
                JobMsg::Done => ended = true,
                JobMsg::Failed(e) => {
                    self.error = Some(e);
                    ended = true;
                }
            }
        }
        if ended {
            self.job = None;
            self.save_cache();
        }
        ended
    }

    /// Engine speed per thread for estimates: measured here, else from the live engine.
    pub fn note_live_speed(&mut self, nps: u64, threads: u32) {
        if self.nps_per_thread.is_none() && nps > 0 {
            self.nps_per_thread = Some(nps as f64 / f64::from(threads.max(1)));
        }
    }

    /// After an inaccuracy, mistake or blunder: how the opponent punishes it and how
    /// the player best answers (shown as arrows, as on Lichess).
    pub fn refutation(&self, node: NodeId, game: &Game) -> Vec<Move> {
        self.analysis.refutation(node, game)
    }

    fn save_cache(&self) {
        if let Some(dir) = cache_dir()
            && let Err(e) = self.analysis.save_cache(&dir)
        {
            eprintln!("review cache: {}: {e}", dir.display());
        }
    }

    // ----- writing into the game -------------------------------------------------

    /// Adds `[%eval]` to every main-line move and, for marked moves, a move-assessment
    /// NAG (unless the author gave one) and a comment; for errors also the better line as
    /// a variation. One undo step. Returns the number of marked moves.
    pub fn write_to_game(&self, game: &mut Game) -> usize {
        game.edit_group(|g| {
            let mut errors = 0;
            for i in 1..self.analysis.line().len() {
                let id = self.analysis.line()[i];
                let Some(node) = g.node(id) else { continue };
                let mut nags = node.nags().to_vec();
                let mut comments: Vec<String> = node
                    .comments_after()
                    .iter()
                    .map(|c| strip_eval(c))
                    .filter(|c| !c.trim().is_empty())
                    .collect();
                if let Some(e) = self.analysis.values()[i]
                    && !matches!(e, Eval::Mate { moves: 0, .. })
                {
                    comments.insert(0, format!("[%eval {}]", e.pgn()));
                }
                if let Some(mark) = self.analysis.marks.get(&id) {
                    // The author's own sign stands; praise over it would contradict it.
                    let authored = nags.iter().any(|n| n.is_move_assessment());
                    if !mark.judgement.is_error() && authored {
                        let _ = g.annotate(id, nags, comments);
                        continue;
                    }
                    errors += 1;
                    if !authored {
                        nags.push(judgement_nag(mark.judgement));
                    }
                    let name = judgement_name(mark.judgement);
                    let text = match &mark.best_label {
                        Some(best) => trf(
                            "{judgement}. {best} was best.",
                            // Written into the game: Latin, as PGN always is.
                            &[("judgement", &name), ("best", best)],
                        ),
                        None => format!("{name}."),
                    };
                    if !comments.contains(&text) {
                        comments.push(text);
                    }
                    if !mark.best.is_empty() {
                        let _ = g.add_line(mark.base, &mark.best[..mark.best.len().min(8)]);
                    }
                }
                let _ = g.annotate(id, nags, comments);
            }
            errors
        })
    }

    // ----- UI --------------------------------------------------------------------

    /// Evaluation graph; returns a main-line node to go to (click or drag).
    pub fn graph(
        &self,
        ui: &mut Ui,
        size: egui::Vec2,
        cursor: NodeId,
        game: &Game,
    ) -> Option<NodeId> {
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        painter.rect_filled(rect, 4.0, visuals.extreme_bg_color);
        let n = self.analysis.values().len();
        if n < 2 {
            return None;
        }
        let inner = rect.shrink2(vec2(6.0, 6.0));
        let x_of = |i: usize| inner.left() + inner.width() * i as f32 / (n - 1) as f32;
        let y_of = |wp: f64| inner.bottom() - inner.height() * (wp / 100.0) as f32;
        let mid = y_of(50.0);
        let wp: Vec<Option<f64>> = self
            .analysis
            .values()
            .iter()
            .map(|v| v.map(|e| e.win_percent(Color::White)))
            .collect();
        let (white_fill, black_fill) = if visuals.dark_mode {
            (Color32::from_gray(150), Color32::from_gray(8))
        } else {
            (Color32::from_gray(255), Color32::from_gray(70))
        };

        // Phase boundaries.
        let phase_line = Stroke::new(1.0, visuals.weak_text_color().gamma_multiply(0.6));
        let font = egui::FontId::proportional(11.0);
        // Labels go right of their boundary, or left of it when they would not fit.
        let label = |x: f32, text: &str| {
            let galley =
                painter.layout_no_wrap(text.to_string(), font.clone(), visuals.weak_text_color());
            let w = galley.size().x;
            let left = if x + 4.0 + w > rect.right() - 4.0 {
                x - 4.0 - w
            } else {
                x + 4.0
            };
            painter.galley(
                pos2(left, rect.top() + 3.0),
                galley,
                visuals.weak_text_color(),
            );
        };
        if self
            .analysis
            .report
            .phases
            .middlegame
            .is_some_and(|m| m > 0)
        {
            label(inner.left(), tr("Opening"));
        }
        for (idx, name) in [
            (self.analysis.report.phases.middlegame, tr("Middlegame")),
            (self.analysis.report.phases.endgame, tr("Endgame")),
        ] {
            if let Some(i) = idx {
                let x = x_of(i);
                if i > 0 {
                    painter.vline(x, rect.y_range(), phase_line);
                }
                label(x, name);
            }
        }

        // Advantage areas between the curve and the middle line.
        for i in 0..n - 1 {
            let (Some(a), Some(b)) = (wp[i], wp[i + 1]) else {
                continue;
            };
            let (x0, x1, y0, y1) = (x_of(i), x_of(i + 1), y_of(a), y_of(b));
            let fill = |pts: Vec<egui::Pos2>, above: bool| {
                painter.add(egui::Shape::convex_polygon(
                    pts,
                    if above { white_fill } else { black_fill },
                    Stroke::NONE,
                ));
            };
            if (a >= 50.0) == (b >= 50.0) {
                fill(
                    vec![pos2(x0, mid), pos2(x0, y0), pos2(x1, y1), pos2(x1, mid)],
                    a >= 50.0,
                );
            } else {
                let t = ((50.0 - a) / (b - a)) as f32;
                let xc = x0 + (x1 - x0) * t;
                fill(vec![pos2(x0, mid), pos2(x0, y0), pos2(xc, mid)], a >= 50.0);
                fill(vec![pos2(xc, mid), pos2(x1, y1), pos2(x1, mid)], b >= 50.0);
            }
        }
        painter.hline(
            inner.x_range(),
            mid,
            Stroke::new(1.0, visuals.weak_text_color().gamma_multiply(0.5)),
        );

        // The curve, broken where positions are not evaluated yet.
        let curve = Stroke::new(1.6, Color32::from_rgb(0xe0, 0x7b, 0x1f));
        let mut run: Vec<egui::Pos2> = Vec::new();
        for (i, v) in wp.iter().enumerate() {
            match v {
                Some(w) => run.push(pos2(x_of(i), y_of(*w))),
                None => {
                    if run.len() > 1 {
                        painter.add(egui::Shape::line(std::mem::take(&mut run), curve));
                    }
                    run.clear();
                }
            }
        }
        if run.len() > 1 {
            painter.add(egui::Shape::line(run, curve));
        }
        for (i, j) in self.analysis.report.judgements.iter().enumerate() {
            if let (Some(j), Some(w)) = (j, wp[i + 1]) {
                painter.circle_filled(pos2(x_of(i + 1), y_of(w)), 3.5, judgement_color(*j));
            }
        }

        // Current move.
        if let Some(ci) = self.line_index(cursor, game) {
            painter.vline(x_of(ci), rect.y_range(), Stroke::new(1.5, curve.color));
        }
        if let Some((done, total)) = self.progress() {
            painter.text(
                rect.right_top() + vec2(-6.0, 3.0),
                egui::Align2::RIGHT_TOP,
                format!("{done}/{total}"),
                egui::FontId::proportional(11.0),
                visuals.text_color(),
            );
        }

        let index_at = |x: f32| {
            let t = ((x - inner.left()) / inner.width()).clamp(0.0, 1.0);
            (t * (n - 1) as f32).round() as usize
        };
        if let Some(p) = resp.hover_pos() {
            let i = index_at(p.x);
            painter.vline(
                x_of(i),
                rect.y_range(),
                Stroke::new(1.0, visuals.weak_text_color()),
            );
            let label = if i == 0 {
                tr("Start position").to_string()
            } else {
                crate::san::show(&game.move_label(self.analysis.line()[i]).unwrap_or_default())
                    .into_owned()
            };
            let eval = self.analysis.values()[i]
                .map(Eval::format)
                .unwrap_or_else(|| "…".into());
            let mark = self
                .analysis
                .report
                .judgements
                .get(i.wrapping_sub(1))
                .copied()
                .flatten();
            resp.clone().on_hover_ui_at_pointer(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(label).strong());
                    ui.label(eval);
                    if let Some(j) = mark {
                        ui.label(
                            RichText::new(judgement_name(j))
                                .color(judgement_text_color(j, ui.visuals().dark_mode)),
                        );
                    }
                });
            });
        }
        if (resp.clicked() || resp.dragged())
            && let Some(p) = resp.interact_pointer_pos()
        {
            let target = self.analysis.line()[index_at(p.x)];
            if target != cursor {
                return Some(target);
            }
        }
        None
    }

    /// Index in the main line of `cursor`, or of the main-line move a variation leaves from.
    fn line_index(&self, cursor: NodeId, game: &Game) -> Option<usize> {
        self.analysis.line_index(cursor, game)
    }

    /// Report tab: per-player statistics in the Lichess layout.
    pub fn report_ui(&mut self, ui: &mut Ui, game: &Game) -> Option<ReportAction> {
        let mut action = None;
        ui.add_space(6.0);
        for color in [Color::White, Color::Black] {
            if let Some(a) = self.side_block(ui, game, color) {
                action = Some(a);
            }
            ui.add_space(10.0);
        }
        ui.separator();
        if let Some(meta) = &self.analysis.meta {
            let source = if meta.nodes == 0 {
                tr("Evaluations from the PGN ([%eval])").to_string()
            } else {
                trf(
                    "{engine} · {nodes} nodes per move",
                    &[
                        ("engine", &meta.engine),
                        ("nodes", &format_count(meta.nodes)),
                    ],
                )
            };
            ui.label(RichText::new(source).small().weak());
            if meta.nodes == 0 {
                ui.label(
                    RichText::new(tr("!! and ! need the engine: the PGN has evaluations only"))
                        .small()
                        .weak(),
                );
            }
        }
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
        }
        let missing = self.missing();
        // An analysis from before !! and !: "Update" looks for them, nothing else.
        let seconds = self.analysis.seconds_missing();
        if (missing > 0 || seconds > 0) && !self.running() {
            ui.horizontal_wrapped(|ui| {
                let text = if missing > 0 {
                    trf("Positions without evaluation: {n}", &[("n", &missing)])
                } else {
                    tr("!! and ! were not looked for in this analysis").to_string()
                };
                ui.label(RichText::new(text).color(ui.visuals().warn_fg_color));
                let can_update = self.analysis.meta.as_ref().is_some_and(|m| m.nodes > 0);
                if can_update && ui.button(tr("Update")).clicked() {
                    action = Some(ReportAction::Update);
                }
            });
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.running() && self.has_data(),
                    egui::Button::new(tr("Write analysis to the game")),
                )
                .on_hover_text(tr(
                    "Adds evaluations, !!/!/?!/?/?? marks, comments and better lines to the game (Ctrl+Z to undo)",
                ))
                .clicked()
            {
                action = Some(ReportAction::Write);
            }
            if ui
                .add_enabled(!self.running(), egui::Button::new(tr("Analyse again…")))
                .clicked()
            {
                action = Some(ReportAction::Reanalyse);
            }
        });
        action
    }

    fn side_block(&mut self, ui: &mut Ui, game: &Game, color: Color) -> Option<ReportAction> {
        let mut action = None;
        let name = game
            .tag(if color == Color::White {
                "White"
            } else {
                "Black"
            })
            .filter(|s| !s.is_empty() && *s != "?")
            .unwrap_or(if color == Color::White {
                tr("White")
            } else {
                tr("Black")
            })
            .to_string();
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            let stroke = Stroke::new(1.5, ui.visuals().text_color());
            if color == Color::White {
                ui.painter()
                    .circle(r.center(), 6.0, Color32::from_gray(235), stroke);
            } else {
                ui.painter()
                    .circle(r.center(), 6.0, Color32::TRANSPARENT, stroke);
            }
            ui.label(RichText::new(name).strong().size(16.0));
        });
        let s = self.analysis.report.side(color).clone();
        let pgn_only = self.analysis.meta.as_ref().is_some_and(|m| m.nodes == 0);
        egui::Grid::new(("review_side", color as u8))
            .num_columns(2)
            .spacing(vec2(10.0, 3.0))
            .show(ui, |ui| {
                for (j, count) in [
                    (Judgement::Brilliant, s.brilliant),
                    (Judgement::Great, s.great),
                    (Judgement::Inaccuracy, s.inaccuracies),
                    (Judgement::Mistake, s.mistakes),
                    (Judgement::Blunder, s.blunders),
                ] {
                    // From the PGN's evaluations alone !! and ! are not known: a dash,
                    // not a zero. A row with nothing in it stays quiet.
                    let unknown = !j.is_error() && pgn_only;
                    let c = if count > 0 {
                        judgement_text_color(j, ui.visuals().dark_mode)
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    let shown = if unknown {
                        "—".to_string()
                    } else {
                        count.to_string()
                    };
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(shown).strong().size(15.0).color(c));
                    });
                    let text = RichText::new(judgement_plural(j)).size(15.0).color(c);
                    if count > 0 {
                        let resp = ui
                            .add(egui::Button::new(text).frame_when_inactive(false))
                            .on_hover_text(tr("Go to the next one"));
                        if resp.clicked()
                            && let Some(id) = self.next_error(color, j)
                        {
                            action = Some(ReportAction::Goto(id));
                        }
                    } else {
                        ui.label(text);
                    }
                    ui.end_row();
                }
                let value_row =
                    |ui: &mut Ui, value: String, label: &str, color: Option<Color32>| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let mut t = RichText::new(value).strong().size(15.0);
                            if let Some(c) = color {
                                t = t.color(c);
                            }
                            ui.label(t);
                        });
                        let mut t = RichText::new(label).size(15.0);
                        if let Some(c) = color {
                            t = t.color(c);
                        }
                        // A long name ("Average centipawn loss") wraps in a narrow panel.
                        ui.add(egui::Label::new(t).wrap());
                        ui.end_row();
                    };
                let dash = || "—".to_string();
                value_row(
                    ui,
                    s.acpl.map_or_else(dash, |v| v.to_string()),
                    tr("Average centipawn loss"),
                    None,
                );
                value_row(
                    ui,
                    s.accuracy.map_or_else(dash, percent),
                    tr("Accuracy"),
                    None,
                );
                for (i, label) in [tr("Opening"), tr("Middlegame"), tr("Endgame")]
                    .into_iter()
                    .enumerate()
                {
                    if let Some(a) = s.phase_accuracy[i] {
                        value_row(
                            ui,
                            percent(a),
                            label,
                            Some(accuracy_color(a, ui.visuals().dark_mode)),
                        );
                    }
                }
            });
        action
    }

    /// Next main-line move of `color` with judgement `j` (cycles).
    fn next_error(&mut self, color: Color, j: Judgement) -> Option<NodeId> {
        let hits = self.analysis.errors(color, j);
        if hits.is_empty() {
            return None;
        }
        let k = self.cycle.entry((color, j)).or_insert(usize::MAX);
        *k = k.wrapping_add(1) % hits.len();
        Some(hits[*k])
    }

    /// Launch dialog (engine, quality, threads, memory, time estimate).
    pub fn dialog(&mut self, ctx: &egui::Context, settings: &mut Settings) -> Option<DialogAction> {
        if !self.dialog_open {
            return None;
        }
        let mut action = None;
        let modal = egui::Modal::new(egui::Id::new("review_dialog")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(tr("Game analysis"));
            ui.label(
                RichText::new(tr(
                    "The engine evaluates every position of the main line. The game itself is not changed.",
                ))
                .weak(),
            );
            ui.add_space(8.0);
            if let Some(a) = engine_picker(ui, settings, "review_engine") {
                action = Some(a);
            }
            ui.add_space(4.0);
            let rs = &mut settings.review;
            let mut quality = rs.quality;
            options_grid(
                ui,
                "review_grid",
                &mut quality,
                &mut rs.threads,
                &mut rs.hash_mb,
                None,
            );
            rs.quality = quality;
            ui.add_space(6.0);
            let positions = self.analysis.positions();
            let per_thread = self.nps_per_thread.unwrap_or(DEFAULT_NPS);
            let searches = self.analysis.searches();
            let secs = estimate_secs(searches, rs.nodes(), per_thread, rs.threads);
            ui.label(trf(
                "{n} positions, about {time}",
                &[("n", &positions), ("time", &format_duration(secs))],
            ));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ready = settings.active_engine().is_some();
                    if ui
                        .add_enabled(
                            ready,
                            egui::Button::new(RichText::new(tr("Analyse")).strong().size(16.0)),
                        )
                        .clicked()
                    {
                        action = Some(DialogAction::Start);
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        self.dialog_open = false;
                    }
                    if !ready {
                        ui.label(
                            RichText::new(tr("Add an engine first"))
                                .color(ui.visuals().warn_fg_color),
                        );
                    }
                });
            });
        });
        if modal.should_close() {
            self.dialog_open = false;
        }
        if matches!(
            action,
            Some(DialogAction::Start | DialogAction::OpenEngines)
        ) {
            self.dialog_open = false;
        }
        action
    }
}

impl Drop for Review {
    fn drop(&mut self) {
        // Leaving the game mid-analysis keeps what was computed.
        self.cancel();
    }
}

/// Nodes per second and thread assumed until an engine was measured.
pub const DEFAULT_NPS: f64 = 700_000.0;

/// Seconds `searches` of `nodes` take at `per_thread` nodes per second on `threads`.
pub fn estimate_secs(searches: usize, nodes: u64, per_thread: f64, threads: u32) -> f64 {
    searches as f64 * nodes as f64 / (per_thread * f64::from(threads.max(1)))
}

/// The «Engine» row of a launch dialog: the active engine, the others to switch to,
/// «Add or edit engines…».
pub fn engine_picker(ui: &mut Ui, settings: &mut Settings, id: &str) -> Option<DialogAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(tr("Engine"));
        let current = settings
            .active_engine()
            .map(|e| e.name.clone())
            .unwrap_or_else(|| tr("not selected").into());
        egui::ComboBox::from_id_salt(id)
            .selected_text(current)
            .width(ui.available_width() - 8.0)
            .show_ui(ui, |ui| {
                for i in 0..settings.engines.len() {
                    let selected = settings.active_engine == Some(i);
                    if ui
                        .selectable_label(selected, &settings.engines[i].name)
                        .clicked()
                        && !selected
                    {
                        settings.active_engine = Some(i);
                        action = Some(DialogAction::EngineSwitch);
                    }
                }
                ui.separator();
                if ui.button(tr("Add or edit engines…")).clicked() {
                    action = Some(DialogAction::OpenEngines);
                }
            });
    });
    action
}

/// The «Quality · Threads · Memory» grid of a launch dialog; `quality` indexes
/// [`REVIEW_QUALITY`]. `quality_hint`: a tooltip on the quality slider.
pub fn options_grid(
    ui: &mut Ui,
    id: &str,
    quality: &mut usize,
    threads: &mut u32,
    hash_mb: &mut u32,
    quality_hint: Option<&str>,
) {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);
    let hash_steps = crate::game_view::hash_steps();
    ui.spacing_mut().slider_width = 200.0;
    egui::Grid::new(id)
        .num_columns(3)
        .spacing(vec2(12.0, 8.0))
        .show(ui, |ui| {
            ui.label(tr("Quality"));
            let mut q = (*quality).min(REVIEW_QUALITY.len() - 1);
            crate::game_view::index_slider(ui, &mut q, REVIEW_QUALITY.len());
            *quality = q;
            let label = ui.label(trf(
                "{quality} · {nodes} nodes",
                &[
                    ("quality", &tr(REVIEW_QUALITY[q].0)),
                    ("nodes", &format_count(REVIEW_QUALITY[q].1)),
                ],
            ));
            if let Some(hint) = quality_hint {
                label.on_hover_text(hint);
            }
            ui.end_row();

            ui.label(tr("Threads"));
            let mut th = (*threads).clamp(1, cores) as usize - 1;
            crate::game_view::index_slider(ui, &mut th, cores as usize);
            *threads = th as u32 + 1;
            ui.label(format!("{} / {cores}", *threads));
            ui.end_row();

            ui.label(tr("Memory"));
            let mut h = hash_steps
                .iter()
                .position(|m| *m >= *hash_mb)
                .unwrap_or(hash_steps.len() - 1);
            crate::game_view::index_slider(ui, &mut h, hash_steps.len());
            *hash_mb = hash_steps[h];
            ui.label(crate::game_view::format_mb(*hash_mb));
            ui.end_row();
        });
}

/// Removes `[%eval …]` commands from a comment.
fn strip_eval(c: &str) -> String {
    let mut out = String::new();
    let mut rest = c;
    while let Some(i) = rest.find("[%eval") {
        out.push_str(&rest[..i]);
        match rest[i..].find(']') {
            Some(j) => rest = &rest[i + j + 1..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// A judgement's color: the same as its sign's NAG (`notation::assessment_color`).
pub fn judgement_color(j: Judgement) -> Color32 {
    crate::notation::assessment_color(judgement_nag(j)).unwrap_or(Color32::GRAY)
}

/// The color of a judgement as text: on a light background darker than the bright one
/// of the badges, as on Android.
pub fn judgement_text_color(j: Judgement, dark: bool) -> Color32 {
    crate::notation::assessment_text_color(judgement_nag(j), dark).unwrap_or(Color32::GRAY)
}

/// The NAG of the same sign.
pub fn judgement_nag(j: Judgement) -> Nag {
    match j {
        Judgement::Inaccuracy => Nag::DUBIOUS_MOVE,
        Judgement::Mistake => Nag::MISTAKE,
        Judgement::Blunder => Nag::BLUNDER,
        Judgement::Great => Nag::GOOD_MOVE,
        Judgement::Brilliant => Nag::BRILLIANT_MOVE,
    }
}

pub fn judgement_symbol(j: Judgement) -> &'static str {
    match j {
        Judgement::Inaccuracy => "?!",
        Judgement::Mistake => "?",
        Judgement::Blunder => "??",
        Judgement::Great => "!",
        Judgement::Brilliant => "!!",
    }
}

pub fn judgement_name(j: Judgement) -> &'static str {
    match j {
        Judgement::Inaccuracy => tr("Inaccuracy"),
        Judgement::Mistake => tr("Mistake"),
        Judgement::Blunder => tr("Blunder"),
        Judgement::Great => tr("Great move"),
        Judgement::Brilliant => tr("Brilliant move"),
    }
}

fn judgement_plural(j: Judgement) -> &'static str {
    match j {
        Judgement::Inaccuracy => tr("Inaccuracies"),
        Judgement::Mistake => tr("Mistakes"),
        Judgement::Blunder => tr("Blunders"),
        Judgement::Great => tr("Great moves"),
        Judgement::Brilliant => tr("Brilliant moves"),
    }
}

/// Accuracy in the colors of the move marks: green, blue, orange, red.
fn accuracy_color(a: f64, dark: bool) -> Color32 {
    let j = if a >= 85.0 {
        Judgement::Great
    } else if a >= 70.0 {
        Judgement::Inaccuracy
    } else if a >= 50.0 {
        Judgement::Mistake
    } else {
        Judgement::Blunder
    };
    judgement_text_color(j, dark)
}

fn percent(a: f64) -> String {
    format!("{}%", a.round() as i64)
}

/// 250 000 → "250k", 1 000 000 → "1M".
pub fn format_count(n: u64) -> String {
    if n >= 1_000_000 && n.is_multiple_of(100_000) {
        let m = n as f64 / 1e6;
        if n.is_multiple_of(1_000_000) {
            format!("{m:.0}M")
        } else {
            format!("{m:.1}M")
        }
    } else if n >= 1000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

pub fn format_duration(secs: f64) -> String {
    let s = secs.ceil().max(1.0) as u64;
    if s < 60 {
        trf("{n} s", &[("n", &s)])
    } else {
        trf("{n} min", &[("n", &s.div_ceil(60))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_eval_keeps_other_text() {
        assert_eq!(strip_eval("[%eval 0.35] Good move"), "Good move");
        assert_eq!(
            strip_eval("A [%eval #-3] B [%clk 0:01:00]"),
            "A  B [%clk 0:01:00]"
        );
        assert_eq!(strip_eval("[%eval 1.0"), "");
    }

    #[test]
    fn counts_are_compact() {
        assert_eq!(format_count(250_000), "250k");
        assert_eq!(format_count(1_000_000), "1M");
        assert_eq!(format_count(1_500_000), "1.5M");
    }

    #[test]
    fn pgn_evals_and_written_analysis_roundtrip() {
        let mut game = chess_core::parse_str(
            "1. e4 {[%eval 0.3]} e5 {[%eval 0.32]} 2. Qh5 {[%eval -0.2]} Ke7 {[%eval 6.5]} \
             3. Qxe5# 1-0",
        )
        .game;
        let mut r = Review::new(&game);
        assert!(r.has_data() && r.visible);
        assert_eq!(r.missing(), 0);
        assert!(r.analysis.report.complete);
        // 2…Ke7 throws the game away; 3.Qxe5# is a terminal position.
        let ke7 = game.mainline()[3];
        assert_eq!(
            r.analysis.marks.get(&ke7).map(|m| m.judgement),
            Some(Judgement::Blunder)
        );
        assert_eq!(r.analysis.report.black.blunders, 1);

        let errors = r.write_to_game(&mut game);
        assert_eq!(errors, r.analysis.marks.len());
        let node = game.node(ke7).expect("node");
        assert!(node.nags().contains(&Nag::BLUNDER));
        assert!(node.comments_after()[0].starts_with("[%eval 6.50]"));
        r.refresh(&game);
        // Writing again replaces evaluations instead of duplicating them.
        r.write_to_game(&mut game);
        let n = game.node(ke7).expect("node");
        assert_eq!(
            n.comments_after()
                .iter()
                .filter(|c| c.contains("[%eval"))
                .count(),
            1
        );
        assert_eq!(
            n.nags().iter().filter(|x| x.is_move_assessment()).count(),
            1
        );
        // Both writes are single undo steps.
        game.undo();
        game.undo();
        assert!(!game.is_modified());
        r.cancel();
    }
}
