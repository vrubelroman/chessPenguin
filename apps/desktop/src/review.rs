//! Whole-game analysis: a background engine job over the main line, cached results,
//! the evaluation graph under the board, the report tab and the launch dialog.
//!
//! The job uses its own engine process and never touches the game; writing the results
//! into the game is a separate, undoable action ([`Review::write_to_game`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use chess_core::shakmaty::san::SanPlus;
use chess_core::shakmaty::uci::UciMove;
use chess_core::shakmaty::{Chess, Color, Move, Position};
use chess_core::{Annotation, CommentPart, Game, Nag, NodeId, parse_comment};
use egui::{Color32, RichText, Sense, Stroke, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};
use uci::{EngineEvent, EnginePosition, ScoreBound, SearchId, SearchLimits, Side};

use crate::i18n::{tr, trf};
use crate::report::{Eval, Judgement, Phases, Ply, Report};
use crate::settings::{EngineConfig, REVIEW_QUALITY, ReviewSettings, Settings};

/// Engine result for one position of the main line.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PosEval {
    pub eval: Eval,
    /// Principal variation in UCI; the first move is the engine's best move.
    #[serde(default)]
    pub pv: Vec<String>,
    #[serde(default)]
    pub depth: u32,
}

/// Where the evaluations came from.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReviewMeta {
    pub engine: String,
    /// Nodes per position; 0 = read from `[%eval]` comments of the PGN.
    pub nodes: u64,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    meta: ReviewMeta,
    evals: Vec<(String, PosEval)>,
}

const CACHE_VERSION: u32 = 1;
/// A node-limited search should never take this long; stop it if it does.
const SEARCH_GUARD: Duration = Duration::from_secs(300);

/// An engine assessment of a main-line move, shown over the notation.
#[derive(Clone, Debug)]
pub struct Mark {
    pub judgement: Judgement,
    /// Node before the move (where the better line starts).
    pub base: NodeId,
    pub best: Vec<Move>,
    pub best_san: Vec<String>,
    /// "23…Nf6".
    pub best_label: Option<String>,
}

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

struct Target {
    key: String,
    pos: EnginePosition,
    turn: Side,
}

enum JobMsg {
    Engine(String),
    Eval {
        key: String,
        eval: PosEval,
        nps: u64,
    },
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
    evals: HashMap<String, PosEval>,
    pub meta: Option<ReviewMeta>,
    job: Option<Job>,
    pub error: Option<String>,
    /// Graph and report tab are shown.
    pub visible: bool,
    pub dialog_open: bool,
    /// Measured engine speed per thread, for time estimates.
    nps_per_thread: Option<f64>,
    built_rev: Option<u64>,
    dirty: bool,
    /// Root and main-line nodes: the analysed positions.
    line: Vec<NodeId>,
    keys: Vec<String>,
    /// Side making each move of the line.
    movers: Vec<Color>,
    /// Evaluation per position (engine result or a terminal position).
    values: Vec<Option<Eval>>,
    pub report: Report,
    pub marks: HashMap<NodeId, Mark>,
    /// Report tab: index of the last visited error per side and kind.
    cycle: HashMap<(Color, Judgement), usize>,
}

impl Review {
    pub fn new(game: &Game) -> Self {
        let mut r = Review {
            evals: HashMap::new(),
            meta: None,
            job: None,
            error: None,
            visible: false,
            dialog_open: false,
            nps_per_thread: None,
            built_rev: None,
            dirty: false,
            line: Vec::new(),
            keys: Vec::new(),
            movers: Vec::new(),
            values: Vec::new(),
            report: Report::default(),
            marks: HashMap::new(),
            cycle: HashMap::new(),
        };
        r.rebuild(game);
        if r.load_cache() || r.load_pgn_evals(game) {
            r.rebuild(game);
            r.visible = true;
        }
        r
    }

    pub fn has_data(&self) -> bool {
        !self.evals.is_empty()
    }

    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    pub fn progress(&self) -> Option<(usize, usize)> {
        self.job.as_ref().map(|j| (j.done, j.total))
    }

    /// Positions of the current main line without an evaluation.
    pub fn missing(&self) -> usize {
        self.values.iter().filter(|v| v.is_none()).count()
    }

    /// Re-derives the report when the game or the evaluations changed. Cheap otherwise.
    pub fn refresh(&mut self, game: &Game) {
        if self.dirty || self.built_rev != Some(game.revision()) {
            self.rebuild(game);
        }
    }

    fn rebuild(&mut self, game: &Game) {
        self.line = std::iter::once(NodeId::ROOT)
            .chain(game.mainline())
            .collect();
        let positions: Vec<Chess> = self
            .line
            .iter()
            .filter_map(|id| game.position(*id).cloned())
            .collect();
        self.keys = self
            .line
            .iter()
            .map(|id| position_key(game, *id).map(|k| k.0).unwrap_or_default())
            .collect();
        self.values = (0..self.line.len())
            .map(|i| {
                self.evals
                    .get(&self.keys[i])
                    .map(|e| e.eval)
                    .or_else(|| positions.get(i).and_then(terminal_eval))
            })
            .collect();
        let mut plies = Vec::with_capacity(self.line.len().saturating_sub(1));
        for i in 1..self.line.len() {
            let mv = game.game_move(self.line[i]);
            let best = self.evals.get(&self.keys[i - 1]).and_then(|e| e.pv.first());
            plies.push(Ply {
                mover: positions.get(i - 1).map_or(Color::White, |p| p.turn()),
                null: mv.is_none_or(|m| m.is_null()),
                best: best.is_some() && mv.and_then(|m| m.uci()).as_ref() == best,
            });
        }
        self.movers = plies.iter().map(|p| p.mover).collect();
        self.report = Report::build(&self.values, &plies, Phases::divide(&positions));
        self.marks.clear();
        for (i, j) in self.report.judgements.iter().enumerate() {
            let Some(judgement) = *j else { continue };
            let (base, node) = (self.line[i], self.line[i + 1]);
            let (best, best_san) = match (self.evals.get(&self.keys[i]), positions.get(i)) {
                (Some(e), Some(root)) => parse_pv(root, &e.pv),
                _ => (Vec::new(), Vec::new()),
            };
            let best_label = best_san.first().and_then(|san| {
                let (num, color) = game.move_number(node)?;
                Some(match color {
                    Color::White => format!("{num}.{san}"),
                    Color::Black => format!("{num}…{san}"),
                })
            });
            self.marks.insert(
                node,
                Mark {
                    judgement,
                    base,
                    best,
                    best_san,
                    best_label,
                },
            );
        }
        self.built_rev = Some(game.revision());
        self.dirty = false;
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
        if full {
            self.evals.clear();
            self.dirty = true;
        }
        self.meta = Some(ReviewMeta {
            engine: engine.name.clone(),
            nodes: rs.nodes(),
        });
        self.rebuild(game);
        let targets: Vec<Target> = (0..self.line.len())
            .filter(|i| self.values[*i].is_none())
            .filter_map(|i| {
                let (key, pos) = position_key(game, self.line[i])?;
                let turn = match game.position(self.line[i])?.turn() {
                    Color::White => Side::White,
                    Color::Black => Side::Black,
                };
                Some(Target { key, pos, turn })
            })
            .collect();
        if targets.is_empty() {
            return false;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let total = targets.len();
        let (path, threads, hash, nodes) = (path, rs.threads.max(1), rs.hash_mb, rs.nodes());
        let (cancel_w, ctx_w) = (cancel.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("game-review".into())
            .spawn(move || {
                let msg = match run_job(path, threads, hash, nodes, targets, &tx, &cancel_w, &ctx_w)
                {
                    Ok(()) => JobMsg::Done,
                    Err(e) => JobMsg::Failed(e),
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
                JobMsg::Engine(name) => {
                    if let Some(m) = &mut self.meta {
                        m.engine = name;
                    }
                }
                JobMsg::Eval { key, eval, nps } => {
                    job.done += 1;
                    if nps > 0 {
                        self.nps_per_thread = Some(nps as f64 / f64::from(job.threads));
                    }
                    self.evals.insert(key, eval);
                    self.dirty = true;
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
    /// the player best answers, the first two moves of the engine line from the position
    /// after the move (shown as arrows, as on Lichess).
    pub fn refutation(&self, node: NodeId, game: &Game) -> Vec<Move> {
        if !self.marks.contains_key(&node) {
            return Vec::new();
        }
        let (Some(i), Some(pos)) = (
            self.line.iter().position(|n| *n == node),
            game.position(node),
        ) else {
            return Vec::new();
        };
        let Some(e) = self.keys.get(i).and_then(|k| self.evals.get(k)) else {
            return Vec::new();
        };
        let (mut moves, _) = parse_pv(pos, &e.pv);
        moves.truncate(2);
        moves
    }

    // ----- cache -----------------------------------------------------------------

    fn cache_path(&self) -> Option<PathBuf> {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for k in &self.keys {
            for b in k.bytes().chain(std::iter::once(b'\n')) {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
        Some(
            Settings::cache_dir()?
                .join("analysis")
                .join(format!("{h:016x}.json")),
        )
    }

    fn save_cache(&self) {
        let Some(meta) = self.meta.clone().filter(|m| m.nodes > 0) else {
            return;
        };
        if self.evals.is_empty() {
            return;
        }
        let Some(path) = self.cache_path() else {
            return;
        };
        let file = CacheFile {
            version: CACHE_VERSION,
            meta,
            evals: self
                .evals
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        match serde_json::to_vec(&file) {
            Ok(bytes) => {
                if let Err(e) = crate::settings::write_atomic(&path, &bytes) {
                    eprintln!("review cache: {}: {e}", path.display());
                }
            }
            Err(e) => eprintln!("review cache: {e}"),
        }
    }

    fn load_cache(&mut self) -> bool {
        let Some(bytes) = self.cache_path().and_then(|p| std::fs::read(p).ok()) else {
            return false;
        };
        let Ok(file) = serde_json::from_slice::<CacheFile>(&bytes) else {
            return false;
        };
        if file.version != CACHE_VERSION || file.evals.is_empty() {
            return false;
        }
        self.meta = Some(file.meta);
        self.evals = file.evals.into_iter().collect();
        true
    }

    /// Uses `[%eval]` comments (Lichess exports, or our own written analysis) when every
    /// main-line move has one.
    fn load_pgn_evals(&mut self, game: &Game) -> bool {
        let mut found = Vec::new();
        for (i, id) in self.line.iter().enumerate().skip(1) {
            let Some(node) = game.node(*id) else {
                return false;
            };
            let e = node
                .comments_after()
                .iter()
                .flat_map(|c| parse_comment(c))
                .find_map(|p| match p {
                    CommentPart::Annotation(Annotation::Other { name, payload })
                        if name == "eval" =>
                    {
                        Eval::parse_pgn(&payload)
                    }
                    _ => None,
                });
            match e {
                Some(e) => found.push((i, e)),
                None if self.values[i].is_some() => {}
                None => return false,
            }
        }
        let Some(&(_, first)) = found.first() else {
            return false;
        };
        // The start position carries no comment: Lichess assumes +0.15 for the initial one.
        let root = if game.start().is_standard() {
            Eval::Cp(15)
        } else {
            first
        };
        found.insert(0, (0, root));
        for (i, eval) in found {
            self.evals.insert(
                self.keys[i].clone(),
                PosEval {
                    eval,
                    pv: Vec::new(),
                    depth: 0,
                },
            );
        }
        self.meta = Some(ReviewMeta {
            engine: "PGN".into(),
            nodes: 0,
        });
        true
    }

    // ----- writing into the game -------------------------------------------------

    /// Adds `[%eval]` to every main-line move and, for errors, a move-assessment NAG
    /// (unless the author gave one), a comment and the better line as a variation.
    /// One undo step. Returns the number of annotated errors.
    pub fn write_to_game(&self, game: &mut Game) -> usize {
        game.edit_group(|g| {
            let mut errors = 0;
            for i in 1..self.line.len() {
                let id = self.line[i];
                let Some(node) = g.node(id) else { continue };
                let mut nags = node.nags().to_vec();
                let mut comments: Vec<String> = node
                    .comments_after()
                    .iter()
                    .map(|c| strip_eval(c))
                    .filter(|c| !c.trim().is_empty())
                    .collect();
                if let Some(e) = self.values[i]
                    && !matches!(e, Eval::Mate { moves: 0, .. })
                {
                    comments.insert(0, format!("[%eval {}]", e.pgn()));
                }
                if let Some(mark) = self.marks.get(&id) {
                    errors += 1;
                    if !nags.iter().any(|n| n.is_move_assessment()) {
                        nags.push(match mark.judgement {
                            Judgement::Inaccuracy => Nag::DUBIOUS_MOVE,
                            Judgement::Mistake => Nag::MISTAKE,
                            Judgement::Blunder => Nag::BLUNDER,
                        });
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
        let n = self.values.len();
        if n < 2 {
            return None;
        }
        let inner = rect.shrink2(vec2(6.0, 6.0));
        let x_of = |i: usize| inner.left() + inner.width() * i as f32 / (n - 1) as f32;
        let y_of = |wp: f64| inner.bottom() - inner.height() * (wp / 100.0) as f32;
        let mid = y_of(50.0);
        let wp: Vec<Option<f64>> = self
            .values
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
        if self.report.phases.middlegame.is_some_and(|m| m > 0) {
            label(inner.left(), tr("Opening"));
        }
        for (idx, name) in [
            (self.report.phases.middlegame, tr("Middlegame")),
            (self.report.phases.endgame, tr("Endgame")),
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
        for (i, j) in self.report.judgements.iter().enumerate() {
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
                crate::san::show(&game.move_label(self.line[i]).unwrap_or_default()).into_owned()
            };
            let eval = self.values[i]
                .map(Eval::format)
                .unwrap_or_else(|| "…".into());
            let mark = self
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
                        ui.label(RichText::new(judgement_name(j)).color(judgement_color(j)));
                    }
                });
            });
        }
        if (resp.clicked() || resp.dragged())
            && let Some(p) = resp.interact_pointer_pos()
        {
            let target = self.line[index_at(p.x)];
            if target != cursor {
                return Some(target);
            }
        }
        None
    }

    /// Index in the main line of `cursor`, or of the main-line move a variation leaves from.
    fn line_index(&self, cursor: NodeId, game: &Game) -> Option<usize> {
        let mut id = cursor;
        loop {
            if let Some(i) = self.line.iter().position(|n| *n == id) {
                return Some(i);
            }
            id = game.parent(id)?;
        }
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
        if let Some(meta) = &self.meta {
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
        }
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
        }
        let missing = self.missing();
        if missing > 0 && !self.running() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(trf("Positions without evaluation: {n}", &[("n", &missing)]))
                        .color(ui.visuals().warn_fg_color),
                );
                let can_update = self.meta.as_ref().is_some_and(|m| m.nodes > 0);
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
                    "Adds evaluations, ?!/?/?? marks, comments and better lines to the game (Ctrl+Z to undo)",
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
        let s = self.report.side(color).clone();
        egui::Grid::new(("review_side", color as u8))
            .num_columns(2)
            .spacing(vec2(10.0, 3.0))
            .show(ui, |ui| {
                for (j, count) in [
                    (Judgement::Inaccuracy, s.inaccuracies),
                    (Judgement::Mistake, s.mistakes),
                    (Judgement::Blunder, s.blunders),
                ] {
                    let c = judgement_color(j);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(count.to_string())
                                .strong()
                                .size(15.0)
                                .color(c),
                        );
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
                        ui.label(t);
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
                        value_row(ui, percent(a), label, Some(accuracy_color(a)));
                    }
                }
            });
        action
    }

    /// Next main-line move of `color` with judgement `j` (cycles).
    fn next_error(&mut self, color: Color, j: Judgement) -> Option<NodeId> {
        let hits: Vec<NodeId> = self
            .report
            .judgements
            .iter()
            .enumerate()
            .filter(|(i, x)| **x == Some(j) && self.movers.get(*i) == Some(&color))
            .map(|(i, _)| self.line[i + 1])
            .collect();
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
            ui.horizontal(|ui| {
                ui.label(tr("Engine"));
                let current = settings
                    .active_engine()
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| tr("not selected").into());
                egui::ComboBox::from_id_salt("review_engine")
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
            ui.add_space(4.0);
            let rs = &mut settings.review;
            let cores = std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1);
            let hash_steps = crate::game_view::hash_steps();
            ui.spacing_mut().slider_width = 200.0;
            egui::Grid::new("review_grid")
                .num_columns(3)
                .spacing(vec2(12.0, 8.0))
                .show(ui, |ui| {
                    ui.label(tr("Quality"));
                    let mut q = rs.quality.min(REVIEW_QUALITY.len() - 1);
                    crate::game_view::index_slider(ui, &mut q, REVIEW_QUALITY.len());
                    rs.quality = q;
                    ui.label(trf(
                        "{quality} · {nodes} nodes",
                        &[
                            ("quality", &tr(REVIEW_QUALITY[q].0)),
                            ("nodes", &format_count(REVIEW_QUALITY[q].1)),
                        ],
                    ));
                    ui.end_row();

                    ui.label(tr("Threads"));
                    let mut th = rs.threads.clamp(1, cores) as usize - 1;
                    crate::game_view::index_slider(ui, &mut th, cores as usize);
                    rs.threads = th as u32 + 1;
                    ui.label(format!("{} / {cores}", rs.threads));
                    ui.end_row();

                    ui.label(tr("Memory"));
                    let mut h = hash_steps
                        .iter()
                        .position(|m| *m >= rs.hash_mb)
                        .unwrap_or(hash_steps.len() - 1);
                    crate::game_view::index_slider(ui, &mut h, hash_steps.len());
                    rs.hash_mb = hash_steps[h];
                    ui.label(crate::game_view::format_mb(rs.hash_mb));
                    ui.end_row();
                });
            ui.add_space(6.0);
            let positions = self
                .values
                .iter()
                .filter(|v| !matches!(v, Some(Eval::Mate { moves: 0, .. })))
                .count();
            let per_thread = self.nps_per_thread.unwrap_or(700_000.0);
            let secs = positions as f64 * rs.nodes() as f64 / (per_thread * f64::from(rs.threads));
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

#[allow(clippy::too_many_arguments)]
fn run_job(
    path: PathBuf,
    threads: u32,
    hash_mb: u32,
    nodes: u64,
    targets: Vec<Target>,
    tx: &Sender<JobMsg>,
    cancel: &AtomicBool,
    ctx: &egui::Context,
) -> Result<(), String> {
    let handle = uci::Engine::spawn_with(uci::EngineConfig::new(&path), Box::new(|| {}))
        .map_err(|e| e.to_string())?;
    if let Some(name) = &handle.info().name {
        let _ = tx.send(JobMsg::Engine(name.clone()));
    }
    // Unknown options are not fatal: some engines lack Threads/Hash/MultiPV.
    let _ = handle.set_threads(threads);
    let _ = handle.set_hash_mb(hash_mb);
    let _ = handle.set_multipv(1);
    let _ = handle.new_game();
    for (i, t) in targets.into_iter().enumerate() {
        let id = SearchId(i as u64 + 1);
        handle
            .search(t.pos.clone(), id, SearchLimits::Nodes(nodes))
            .map_err(|e| e.to_string())?;
        let started = Instant::now();
        let mut stopping = false;
        let mut last: Option<uci::SearchInfo> = None;
        loop {
            if cancel.load(Ordering::Relaxed) {
                handle.quit();
                return Ok(());
            }
            if !stopping && started.elapsed() > SEARCH_GUARD {
                let _ = handle.stop();
                stopping = true;
            }
            match handle.recv_timeout(Duration::from_millis(100)) {
                Some(EngineEvent::Info { search_id, info }) if search_id == id => {
                    if info.multipv_index() == 1 && info.score.is_some() {
                        let exact = info.score.is_some_and(|s| s.bound == ScoreBound::Exact);
                        if exact || last.is_none() {
                            last = Some(info);
                        }
                    }
                }
                Some(EngineEvent::BestMove {
                    search_id, best, ..
                }) if search_id == id => {
                    if let Some(info) = last.take()
                        && let Some(score) = info.score
                    {
                        let eval = Eval::from_white_eval(score.to_white(t.turn).eval);
                        let mut pv: Vec<String> = info.pv.iter().take(12).cloned().collect();
                        if let Some(b) = best
                            && pv.first() != Some(&b)
                        {
                            pv = vec![b];
                        }
                        let _ = tx.send(JobMsg::Eval {
                            key: t.key,
                            eval: PosEval {
                                eval,
                                pv,
                                depth: info.depth.unwrap_or(0),
                            },
                            nps: info.nps.unwrap_or(0),
                        });
                        ctx.request_repaint();
                    }
                    break;
                }
                Some(EngineEvent::Terminated(reason)) => {
                    return Err(crate::analysis::termination_message(&reason));
                }
                _ => {}
            }
        }
    }
    handle.quit();
    Ok(())
}

/// Cache key and UCI position of a node (full history, see [`Game::engine_history`]).
fn position_key(game: &Game, id: NodeId) -> Option<(String, EnginePosition)> {
    let h = game.engine_history(id)?;
    let key = format!(
        "{}|{}",
        h.start_fen.as_deref().unwrap_or("startpos"),
        h.moves.join(" ")
    );
    let pos = match h.start_fen {
        None => EnginePosition::startpos(h.moves),
        Some(fen) => EnginePosition::fen(fen, h.moves),
    };
    Some((key, pos))
}

/// Evaluation of a position without legal moves.
fn terminal_eval(pos: &Chess) -> Option<Eval> {
    if pos.is_checkmate() {
        Some(Eval::Mate {
            white: pos.turn() == Color::Black,
            moves: 0,
        })
    } else if pos.is_stalemate() {
        Some(Eval::Cp(0))
    } else {
        None
    }
}

fn parse_pv(root: &Chess, pv: &[String]) -> (Vec<Move>, Vec<String>) {
    let mut pos = root.clone();
    let (mut moves, mut san) = (Vec::new(), Vec::new());
    for u in pv {
        let Some(m) = u
            .parse::<UciMove>()
            .ok()
            .and_then(|um| um.to_move(&pos).ok())
        else {
            break;
        };
        san.push(SanPlus::from_move_and_play_unchecked(&mut pos, m).to_string());
        moves.push(m);
    }
    (moves, san)
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

pub fn judgement_color(j: Judgement) -> Color32 {
    match j {
        Judgement::Inaccuracy => Color32::from_rgb(0x56, 0xb4, 0xe9),
        Judgement::Mistake => Color32::from_rgb(0xe6, 0x8f, 0x00),
        Judgement::Blunder => Color32::from_rgb(0xdf, 0x53, 0x53),
    }
}

pub fn judgement_symbol(j: Judgement) -> &'static str {
    match j {
        Judgement::Inaccuracy => "?!",
        Judgement::Mistake => "?",
        Judgement::Blunder => "??",
    }
}

pub fn judgement_name(j: Judgement) -> &'static str {
    match j {
        Judgement::Inaccuracy => tr("Inaccuracy"),
        Judgement::Mistake => tr("Mistake"),
        Judgement::Blunder => tr("Blunder"),
    }
}

fn judgement_plural(j: Judgement) -> &'static str {
    match j {
        Judgement::Inaccuracy => tr("Inaccuracies"),
        Judgement::Mistake => tr("Mistakes"),
        Judgement::Blunder => tr("Blunders"),
    }
}

fn accuracy_color(a: f64) -> Color32 {
    if a >= 85.0 {
        Color32::from_rgb(0x62, 0x9c, 0x2e)
    } else if a >= 70.0 {
        Color32::from_rgb(0x56, 0xb4, 0xe9)
    } else if a >= 50.0 {
        Color32::from_rgb(0xe6, 0x8f, 0x00)
    } else {
        Color32::from_rgb(0xdf, 0x53, 0x53)
    }
}

fn percent(a: f64) -> String {
    format!("{}%", a.round() as i64)
}

/// 250 000 → "250k", 1 000 000 → "1M".
fn format_count(n: u64) -> String {
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

fn format_duration(secs: f64) -> String {
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
        assert!(r.report.complete);
        // 2…Ke7 throws the game away; 3.Qxe5# is a terminal position.
        let ke7 = game.mainline()[3];
        assert_eq!(
            r.marks.get(&ke7).map(|m| m.judgement),
            Some(Judgement::Blunder)
        );
        assert_eq!(r.report.black.blunders, 1);

        let errors = r.write_to_game(&mut game);
        assert_eq!(errors, r.marks.len());
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
