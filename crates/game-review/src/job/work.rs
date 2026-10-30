//! What the job does for one target, as a machine without I/O: it names the next
//! search (the full one, the second best without the move played, the shallow ones)
//! and takes its answer; at the end it gives the target's [`JobEvent::Eval`]. Engine
//! processes on a computer and Web Workers in a browser drive the same machine, so the
//! evaluations — and the marks — are the same everywhere.

use uci::{EnginePosition, SearchInfo, Side};

use super::{JobEvent, Target};
use crate::{Eval, PosEval, Second, Shallow};

/// A search's answer: its last full line and the best move; `None` when it gave none.
pub(crate) type Answer = Option<(SearchInfo, Option<String>)>;

/// What the machine wants now.
pub(crate) enum Next {
    /// A node-limited search of `pos` (among these moves only; empty: all), from a
    /// clear hash.
    Search {
        pos: EnginePosition,
        among: Vec<String>,
        nodes: u64,
    },
    /// The target is done: its evaluation (`None`: the engine gave nothing to use).
    Done(Option<JobEvent>),
}

enum Stage {
    Full,
    Second,
    ShallowBest,
    ShallowPlayed { best: String, best_eval: Eval },
    Finish,
}

pub(crate) struct Work {
    t: Target,
    nodes: u64,
    stage: Stage,
    eval: Option<PosEval>,
    nps: u64,
    /// The move played, when it is the engine's best and worth a second search.
    played: Option<String>,
}

impl Work {
    pub(crate) fn new(t: Target, nodes: u64) -> Self {
        let mut w = Work {
            t,
            nodes,
            stage: Stage::Full,
            eval: None,
            nps: 0,
            played: None,
        };
        if let Some(e) = w.t.known.clone() {
            w.got_eval(e, 0);
        }
        w
    }

    fn eval_of(&self, info: &SearchInfo) -> Option<Eval> {
        info.score
            .map(|s| Eval::from_white_eval(s.to_white(self.t.turn).eval))
    }

    /// The full evaluation is there: the second search, if it is worth one.
    fn got_eval(&mut self, eval: PosEval, nps: u64) {
        let t = &self.t;
        // The move played is the engine's best: what the best of the others gives says
        // whether it was the only good move, or a sacrifice when nothing else won.
        let mover = match t.turn {
            Side::White => chess_core::shakmaty::Color::White,
            Side::Black => chess_core::shakmaty::Color::Black,
        };
        self.played = t
            .played
            .as_ref()
            .filter(|p| {
                eval.pv.first() == Some(*p)
                    && !t.others.is_empty()
                    && crate::report::worth_second(eval.eval, mover)
            })
            .cloned();
        let second_known = eval.second.as_ref().map(|s| &s.without) == self.played.as_ref();
        self.eval = Some(eval);
        self.nps = nps;
        self.stage = if self.played.is_some() && !second_known {
            Stage::Second
        } else {
            Stage::ShallowBest
        };
        if matches!(self.stage, Stage::ShallowBest) {
            self.after_second();
        }
    }

    /// The shallow search, when the rules want one and it is not there yet.
    fn after_second(&mut self) {
        let wanted = match (&self.played, self.t.shallow, &self.eval) {
            (Some(played), Some(nodes), Some(eval)) => !eval
                .shallow
                .as_ref()
                .is_some_and(|s| &s.played == played && s.nodes == nodes),
            _ => false,
        };
        self.stage = if wanted {
            Stage::ShallowBest
        } else {
            Stage::Finish
        };
    }

    pub(crate) fn next(&self) -> Next {
        let t = &self.t;
        match &self.stage {
            Stage::Full => Next::Search {
                pos: t.pos.clone(),
                among: Vec::new(),
                nodes: self.nodes,
            },
            Stage::Second => Next::Search {
                pos: t.pos.clone(),
                among: t.others.clone(),
                nodes: self.nodes,
            },
            Stage::ShallowBest => Next::Search {
                pos: t.pos.clone(),
                among: Vec::new(),
                nodes: t.shallow.unwrap_or_default(),
            },
            Stage::ShallowPlayed { .. } => Next::Search {
                pos: t.pos.clone(),
                among: self.played.iter().cloned().collect(),
                nodes: t.shallow.unwrap_or_default(),
            },
            Stage::Finish => Next::Done(self.eval.clone().map(|eval| JobEvent::Eval {
                key: t.key.clone(),
                eval,
                nps: self.nps,
            })),
        }
    }

    /// The answer to the search [`Self::next`] asked for.
    pub(crate) fn answer(&mut self, answer: Answer) {
        match std::mem::replace(&mut self.stage, Stage::Finish) {
            Stage::Full => {
                let Some((info, best)) = answer else { return };
                let Some(score) = info.score else { return };
                let mut pv: Vec<String> = info.pv.iter().take(12).cloned().collect();
                if let Some(b) = best
                    && pv.first() != Some(&b)
                {
                    pv = vec![b];
                }
                let eval = PosEval {
                    eval: Eval::from_white_eval(score.to_white(self.t.turn).eval),
                    pv,
                    depth: info.depth.unwrap_or(0),
                    second: None,
                    shallow: None,
                };
                self.got_eval(eval, info.nps.unwrap_or(0));
            }
            Stage::Second => {
                let played = self.played.clone();
                // An engine that ignores `searchmoves` answers with the move played: no
                // second move then.
                if let Some((info, best)) = answer
                    && best.as_ref() != played.as_ref()
                    && info.pv.first() != played.as_ref()
                    && let (Some(score), Some(played), Some(eval)) =
                        (info.score, played, self.eval.as_mut())
                {
                    eval.second = Some(Second {
                        without: played,
                        eval: Eval::from_white_eval(score.to_white(self.t.turn).eval),
                    });
                }
                self.after_second();
            }
            Stage::ShallowBest => {
                let Some((info, best)) = answer else { return };
                let (Some(best), Some(best_eval)) = (
                    best.or_else(|| info.pv.first().cloned()),
                    self.eval_of(&info),
                ) else {
                    return;
                };
                if self.played.as_ref() == Some(&best) {
                    self.shallow(best, best_eval, best_eval);
                } else {
                    self.stage = Stage::ShallowPlayed { best, best_eval };
                }
            }
            Stage::ShallowPlayed { best, best_eval } => {
                if let Some((info, b)) = answer
                    && b.as_deref()
                        .is_none_or(|b| Some(b) == self.played.as_deref())
                    && let Some(played_eval) = self.eval_of(&info)
                {
                    self.shallow(best, best_eval, played_eval);
                }
            }
            Stage::Finish => {}
        }
    }

    fn shallow(&mut self, best: String, best_eval: Eval, played_eval: Eval) {
        if let (Some(eval), Some(played), Some(nodes)) =
            (self.eval.as_mut(), self.played.clone(), self.t.shallow)
        {
            eval.shallow = Some(Shallow {
                nodes,
                played,
                best,
                best_eval,
                played_eval,
            });
        }
    }
}
