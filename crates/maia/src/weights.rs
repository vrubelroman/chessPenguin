//! The weights file: `MAIA3W02`, a u32 count, then per tensor a u16 name length, the
//! name, a u8 rank, u32 dims and a u64 offset; the data are f16 little-endian.

use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum WeightsError {
    #[error("cannot read the weights: {0}")]
    Io(std::io::Error),
    #[error("not a Maia weights file")]
    Format,
    #[error("the weights lack {0}")]
    Missing(String),
    #[error("{0} has the shape {1:?}, expected {2:?}")]
    Shape(String, Vec<usize>, Vec<usize>),
}

pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

pub struct Weights {
    tensors: HashMap<String, Tensor>,
}

fn f16_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1f);
    let frac = u32::from(h & 0x3ff);
    let bits = match (exp, frac) {
        (0, 0) => sign,
        (0, f) => {
            // Subnormal: normalise.
            let mut e = 127 - 15 + 1;
            let mut f = f;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((f & 0x3ff) << 13)
        }
        (0x1f, f) => sign | 0x7f80_0000 | (f << 13),
        (e, f) => sign | ((e + 127 - 15) << 23) | (f << 13),
    };
    f32::from_bits(bits)
}

impl Weights {
    pub fn parse(b: &[u8]) -> Result<Weights, WeightsError> {
        let take = |at: usize, n: usize| b.get(at..at + n).ok_or(WeightsError::Format);
        if take(0, 8)? != b"MAIA3W02" {
            return Err(WeightsError::Format);
        }
        let count = u32::from_le_bytes(take(8, 4)?.try_into().unwrap()) as usize;
        let mut at = 12;
        let mut tensors = HashMap::new();
        for _ in 0..count {
            let len = u16::from_le_bytes(take(at, 2)?.try_into().unwrap()) as usize;
            at += 2;
            let name = std::str::from_utf8(take(at, len)?)
                .map_err(|_| WeightsError::Format)?
                .to_owned();
            at += len;
            let rank = take(at, 1)?[0] as usize;
            at += 1;
            let mut shape = Vec::with_capacity(rank);
            for _ in 0..rank {
                shape.push(u32::from_le_bytes(take(at, 4)?.try_into().unwrap()) as usize);
                at += 4;
            }
            let offset = u64::from_le_bytes(take(at, 8)?.try_into().unwrap()) as usize;
            at += 8;
            let n: usize = shape.iter().product();
            let raw = take(offset, 2 * n)?;
            let data = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| f16_to_f32(u16::from_le_bytes(*c)))
                .collect();
            tensors.insert(name, Tensor { shape, data });
        }
        Ok(Weights { tensors })
    }

    /// The tensor `name` of this shape.
    pub fn get(&self, name: &str, shape: &[usize]) -> Result<Vec<f32>, WeightsError> {
        let t = self
            .tensors
            .get(name)
            .ok_or_else(|| WeightsError::Missing(name.to_owned()))?;
        if t.shape != shape {
            return Err(WeightsError::Shape(
                name.to_owned(),
                t.shape.clone(),
                shape.to_vec(),
            ));
        }
        Ok(t.data.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::f16_to_f32;

    #[test]
    fn halves() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert!((f16_to_f32(0x3555) - 0.33325).abs() < 1e-4);
        assert!((f16_to_f32(0x0001) - 5.96e-8).abs() < 1e-9);
    }
}
