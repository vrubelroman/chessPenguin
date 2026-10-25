//! The forward pass of Maia-3 "5M": 8 post-norm transformer blocks over the 64 squares,
//! with an attention bias generated from the position (GAB), a source-destination
//! policy head and a win/draw/loss head. Plain loops over f32 slices.

use shakmaty::uci::UciMove;
use shakmaty::{CastlingMode, Chess, Color, Move, Position, Role, Square};

use crate::weights::{Weights, WeightsError};

const D: usize = 256; // width of the squares' vectors
const HEADS: usize = 8;
const HD: usize = D / HEADS;
const FF: usize = 512;
const EMB: usize = 128; // rating embedding
const HISTORY: usize = 8;
const PLANES: usize = 12;
const INPUT: usize = PLANES * HISTORY + 2 * EMB;
const GAB_HID: usize = 64;
const GAB_GEN: usize = 64;
const HEAD: usize = 256;
const BLOCKS: usize = 8;

struct Linear {
    w: Vec<f32>, // out × in
    b: Option<Vec<f32>>,
    inp: usize,
    out: usize,
}

impl Linear {
    fn load(
        w: &Weights,
        name: &str,
        out: usize,
        inp: usize,
        bias: bool,
    ) -> Result<Linear, WeightsError> {
        Ok(Linear {
            w: w.get(&format!("{name}.weight"), &[out, inp])?,
            b: if bias {
                Some(w.get(&format!("{name}.bias"), &[out])?)
            } else {
                None
            },
            inp,
            out,
        })
    }

    /// `rows` vectors of `inp` → `rows` vectors of `out`.
    fn apply(&self, x: &[f32], rows: usize) -> Vec<f32> {
        let mut y = vec![0.0; rows * self.out];
        for r in 0..rows {
            let xr = &x[r * self.inp..(r + 1) * self.inp];
            let yr = &mut y[r * self.out..(r + 1) * self.out];
            for (o, slot) in yr.iter_mut().enumerate() {
                let wo = &self.w[o * self.inp..(o + 1) * self.inp];
                *slot = dot(xr, wo) + self.b.as_ref().map_or(0.0, |b| b[o]);
            }
        }
        y
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    // Eight running sums: the compiler turns this into vector code.
    let mut s = [0.0f32; 8];
    let chunks = a.len() / 8;
    for i in 0..chunks {
        for k in 0..8 {
            s[k] += a[i * 8 + k] * b[i * 8 + k];
        }
    }
    let mut t: f32 = s.iter().sum();
    for i in chunks * 8..a.len() {
        t += a[i] * b[i];
    }
    t
}

struct LayerNorm {
    w: Vec<f32>,
    b: Vec<f32>,
}

impl LayerNorm {
    fn load(w: &Weights, name: &str, n: usize) -> Result<LayerNorm, WeightsError> {
        Ok(LayerNorm {
            w: w.get(&format!("{name}.weight"), &[n])?,
            b: w.get(&format!("{name}.bias"), &[n])?,
        })
    }

    fn apply(&self, x: &mut [f32]) {
        let n = self.w.len();
        for row in x.chunks_mut(n) {
            let mean = row.iter().sum::<f32>() / n as f32;
            let var = row.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n as f32;
            let inv = 1.0 / (var + 1e-5).sqrt();
            for (i, v) in row.iter_mut().enumerate() {
                *v = (*v - mean) * inv * self.w[i] + self.b[i];
            }
        }
    }
}

fn rms_norm(x: &mut [f32], w: &[f32]) {
    let n = w.len();
    for row in x.chunks_mut(n) {
        let ms = row.iter().map(|v| v * v).sum::<f32>() / n as f32;
        let inv = 1.0 / (ms + f32::EPSILON).sqrt();
        for (i, v) in row.iter_mut().enumerate() {
            *v *= inv * w[i];
        }
    }
}

/// The error function (Abramowitz–Stegun 7.1.26 refined: |error| < 1.2e-7).
fn erf(x: f32) -> f32 {
    let x = f64::from(x);
    let t = 1.0 / (1.0 + 0.5 * x.abs());
    let y = 1.0
        - t * (-x * x - 1.265_512_23
            + t * (1.000_023_68
                + t * (0.374_091_96
                    + t * (0.096_784_18
                        + t * (-0.186_288_06
                            + t * (0.278_868_07
                                + t * (-1.135_203_98
                                    + t * (1.488_515_87
                                        + t * (-0.822_152_23 + t * 0.170_872_77)))))))))
            .exp();
    (if x >= 0.0 { y } else { -y }) as f32
}

fn gelu(x: &mut [f32]) {
    for v in x {
        *v = 0.5 * *v * (1.0 + erf(*v / std::f32::consts::SQRT_2));
    }
}

struct Block {
    in_proj: Linear,  // 3D × D, no bias
    out_proj: Linear, // D × D, no bias
    gab_in: Linear,   // sm2: D → GAB_HID
    gab_ln1: LayerNorm,
    gab_out: Linear, // sm3: GAB_HID → HEADS × GAB_GEN
    gab_ln2: LayerNorm,
    ff1: Linear,
    ff2: Linear,
    norm1: Vec<f32>,
    norm2: Vec<f32>,
}

pub struct Net {
    elo_low: Vec<f32>,
    elo_high: Vec<f32>,
    projection: Linear,
    gab_weight: Vec<f32>, // 4096 × GAB_GEN, shared by the blocks
    blocks: Vec<Block>,
    final_norm: LayerNorm,
    last_ln: LayerNorm,
    value_hid: Linear,
    value: Linear,
    sq_from: Linear,
    sq_to: Linear,
    promo: Linear,
}

/// The moves with their probabilities (most likely first) and the chances of the side
/// to move: win, draw, loss.
#[derive(Clone, Debug, Default)]
pub struct Prediction {
    pub moves: Vec<(Move, f32)>,
    pub wdl: [f32; 3],
}

impl Net {
    pub fn new(w: &Weights) -> Result<Net, WeightsError> {
        let mut blocks = Vec::with_capacity(BLOCKS);
        for i in 0..BLOCKS {
            let p = format!("transformer.layers.{i}");
            blocks.push(Block {
                in_proj: Linear {
                    w: w.get(&format!("{p}.self_attn.mha.in_proj_weight"), &[3 * D, D])?,
                    b: None,
                    inp: D,
                    out: 3 * D,
                },
                out_proj: Linear::load(w, &format!("{p}.self_attn.mha.out_proj"), D, D, false)?,
                gab_in: Linear::load(w, &format!("{p}.self_attn.sm2"), GAB_HID, D, true)?,
                gab_ln1: LayerNorm::load(w, &format!("{p}.self_attn.ln1"), GAB_HID)?,
                gab_out: Linear::load(
                    w,
                    &format!("{p}.self_attn.sm3"),
                    HEADS * GAB_GEN,
                    GAB_HID,
                    true,
                )?,
                gab_ln2: LayerNorm::load(w, &format!("{p}.self_attn.ln2"), HEADS * GAB_GEN)?,
                ff1: Linear::load(w, &format!("{p}.linear1"), FF, D, true)?,
                ff2: Linear::load(w, &format!("{p}.linear2"), D, FF, true)?,
                norm1: w.get(&format!("{p}.norm1.weight"), &[D])?,
                norm2: w.get(&format!("{p}.norm2.weight"), &[D])?,
            });
        }
        Ok(Net {
            elo_low: w.get("elo_embedding_low.weight", &[1, EMB])?,
            elo_high: w.get("elo_embedding_high.weight", &[1, EMB])?,
            projection: Linear::load(w, "token_projection", D, INPUT, true)?,
            gab_weight: w.get("gab_shared_weight", &[64 * 64, GAB_GEN])?,
            blocks,
            final_norm: LayerNorm::load(w, "transformer.norm", D)?,
            last_ln: LayerNorm::load(w, "last_ln", D)?,
            value_hid: Linear::load(w, "fc_value_hid", HEAD, D, true)?,
            value: Linear::load(w, "fc_value", 3, HEAD, true)?,
            sq_from: Linear::load(w, "proj_sq_from", HEAD, D, false)?,
            sq_to: Linear::load(w, "proj_sq_to", HEAD, D, false)?,
            promo: Linear::load(w, "promo_bias_proj", 4, HEAD, false)?,
        })
    }

    fn elo(&self, elo: u32) -> Vec<f32> {
        let low = elo.min(5000) as f32 / 5000.0;
        (0..EMB)
            .map(|i| low * self.elo_low[i] + (1.0 - low) * self.elo_high[i])
            .collect()
    }

    pub fn predict(&self, history: &[Chess], self_elo: u32, oppo_elo: u32) -> Prediction {
        let Some(pos) = history.last() else {
            return Prediction::default();
        };
        let legal = pos.legal_moves();
        if legal.is_empty() {
            return Prediction::default();
        }
        // The input: per square, 12 planes of each of the last 8 positions (each seen
        // from its own side to move; the earliest repeated in front when there are
        // fewer), then the two ratings.
        let start = history.len().saturating_sub(HISTORY);
        let shown = &history[start..];
        let mut x = vec![0.0f32; 64 * INPUT];
        for slot in 0..HISTORY {
            let p = &shown[slot.saturating_sub(HISTORY - shown.len())];
            let turn = p.turn();
            for (sq, piece) in p.board().clone() {
                let s = if turn == Color::Black {
                    sq.flip_vertical()
                } else {
                    sq
                };
                let role = match piece.role {
                    Role::Pawn => 0,
                    Role::Knight => 1,
                    Role::Bishop => 2,
                    Role::Rook => 3,
                    Role::Queen => 4,
                    Role::King => 5,
                };
                let plane = role + if piece.color == turn { 0 } else { 6 };
                x[usize::from(s) * INPUT + slot * PLANES + plane] = 1.0;
            }
        }
        let (se, oe) = (self.elo(self_elo), self.elo(oppo_elo));
        for sq in 0..64 {
            let row = &mut x[sq * INPUT + PLANES * HISTORY..(sq + 1) * INPUT];
            row[..EMB].copy_from_slice(&se);
            row[EMB..].copy_from_slice(&oe);
        }
        let mut h = self.projection.apply(&x, 64);
        for b in &self.blocks {
            h = self.block(b, h);
        }
        self.final_norm.apply(&mut h);

        // Moves: from-square × to-square scores, and promotions with a bias per piece.
        let from = self.sq_from.apply(&h, 64);
        let to = self.sq_to.apply(&h, 64);
        let scale = (HEAD as f32).sqrt();
        let score = |i: usize, j: usize| {
            dot(
                &from[i * HEAD..(i + 1) * HEAD],
                &to[j * HEAD..(j + 1) * HEAD],
            ) / scale
        };
        let promo_bias = self.promo.apply(&to[56 * HEAD..64 * HEAD], 8); // 8 × 4
        let black = pos.turn() == Color::Black;
        let mut logits: Vec<(Move, f32)> = Vec::with_capacity(legal.len());
        for m in &legal {
            let UciMove::Normal {
                from: f,
                to: t,
                promotion,
            } = m.to_uci(CastlingMode::Standard)
            else {
                continue;
            };
            let flip = |s: Square| if black { s.flip_vertical() } else { s };
            let (f, t) = (usize::from(flip(f)), usize::from(flip(t)));
            let logit = match promotion {
                None => score(f, t),
                Some(role) => {
                    let piece = match role {
                        Role::Queen => 0,
                        Role::Rook => 1,
                        Role::Bishop => 2,
                        _ => 3,
                    };
                    score(f, t) + promo_bias[(t - 56) * 4 + piece] * scale
                }
            };
            logits.push((*m, logit));
        }
        let max = logits
            .iter()
            .map(|(_, l)| *l)
            .fold(f32::NEG_INFINITY, f32::max);
        let sum: f32 = logits.iter().map(|(_, l)| (l - max).exp()).sum();
        let mut moves: Vec<(Move, f32)> = logits
            .into_iter()
            .map(|(m, l)| (m, (l - max).exp() / sum))
            .collect();
        moves.sort_by(|a, b| b.1.total_cmp(&a.1));

        // Win, draw, loss of the side to move (the network gives loss, draw, win).
        let mut mean = vec![0.0f32; D];
        for row in h.chunks(D) {
            for (m, v) in mean.iter_mut().zip(row) {
                *m += v / 64.0;
            }
        }
        self.last_ln.apply(&mut mean);
        let mut hid = self.value_hid.apply(&mean, 1);
        for v in &mut hid {
            *v = v.max(0.0);
        }
        let v = self.value.apply(&hid, 1);
        let vmax = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let e: Vec<f32> = v.iter().map(|x| (x - vmax).exp()).collect();
        let s: f32 = e.iter().sum();
        Prediction {
            moves,
            wdl: [e[2] / s, e[1] / s, e[0] / s],
        }
    }

    /// One post-norm block: attention with the generated bias, then the feed-forward.
    fn block(&self, b: &Block, x: Vec<f32>) -> Vec<f32> {
        // The attention bias from the mean of the squares.
        let mut mean = vec![0.0f32; D];
        for row in x.chunks(D) {
            for (m, v) in mean.iter_mut().zip(row) {
                *m += v / 64.0;
            }
        }
        let mut g = b.gab_in.apply(&mean, 1);
        gelu(&mut g);
        b.gab_ln1.apply(&mut g);
        let mut g = b.gab_out.apply(&g, 1);
        gelu(&mut g);
        b.gab_ln2.apply(&mut g); // HEADS × GAB_GEN
        // bias[h][q][k] = g[h] · gab_weight[q*64 + k]
        let mut bias = vec![0.0f32; HEADS * 64 * 64];
        for hh in 0..HEADS {
            let gh = &g[hh * GAB_GEN..(hh + 1) * GAB_GEN];
            for o in 0..64 * 64 {
                bias[hh * 4096 + o] = dot(gh, &self.gab_weight[o * GAB_GEN..(o + 1) * GAB_GEN]);
            }
        }

        let qkv = b.in_proj.apply(&x, 64); // 64 × 3D
        let mut att = vec![0.0f32; 64 * D];
        let scale = 1.0 / (HD as f32).sqrt();
        let mut q = vec![0.0f32; HD];
        let mut scores = vec![0.0f32; 64];
        let mut kh = vec![0.0f32; 64 * HD];
        let mut vh = vec![0.0f32; 64 * HD];
        for hh in 0..HEADS {
            for j in 0..64 {
                kh[j * HD..(j + 1) * HD]
                    .copy_from_slice(&qkv[j * 3 * D + D + hh * HD..j * 3 * D + D + (hh + 1) * HD]);
                vh[j * HD..(j + 1) * HD].copy_from_slice(
                    &qkv[j * 3 * D + 2 * D + hh * HD..j * 3 * D + 2 * D + (hh + 1) * HD],
                );
            }
            for i in 0..64 {
                q.copy_from_slice(&qkv[i * 3 * D + hh * HD..i * 3 * D + (hh + 1) * HD]);
                let mut max = f32::NEG_INFINITY;
                for j in 0..64 {
                    let s =
                        dot(&q, &kh[j * HD..(j + 1) * HD]) * scale + bias[hh * 4096 + i * 64 + j];
                    scores[j] = s;
                    max = max.max(s);
                }
                let mut sum = 0.0;
                for s in scores.iter_mut() {
                    *s = (*s - max).exp();
                    sum += *s;
                }
                let out = &mut att[i * D + hh * HD..i * D + (hh + 1) * HD];
                for j in 0..64 {
                    let w = scores[j] / sum;
                    for (o, v) in out.iter_mut().zip(&vh[j * HD..(j + 1) * HD]) {
                        *o += w * v;
                    }
                }
            }
        }
        let sa = b.out_proj.apply(&att, 64);
        let mut x: Vec<f32> = x.iter().zip(&sa).map(|(a, b)| a + b).collect();
        rms_norm(&mut x, &b.norm1);
        let mut f = b.ff1.apply(&x, 64);
        gelu(&mut f);
        let f = b.ff2.apply(&f, 64);
        let mut x: Vec<f32> = x.iter().zip(&f).map(|(a, b)| a + b).collect();
        rms_norm(&mut x, &b.norm2);
        x
    }
}
