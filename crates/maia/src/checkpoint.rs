//! Reads the published PyTorch checkpoint (`maia3-5m.pt`) directly: a zip archive with
//! stored (uncompressed) members — a pickle describing the tensors (`data.pkl`) and
//! their raw little-endian f32 storages (`data/<key>`). Only what such a checkpoint
//! uses is understood; anything else is refused, nothing is executed.

use std::collections::HashMap;

use crate::weights::{Tensor, WeightsError};

/// The members of a zip archive stored without compression: name → bytes.
fn zip_members(b: &[u8]) -> Result<HashMap<String, &[u8]>, WeightsError> {
    let u16_at = |i: usize| -> Result<usize, WeightsError> {
        Ok(u16::from_le_bytes(
            b.get(i..i + 2)
                .ok_or(WeightsError::Format)?
                .try_into()
                .unwrap(),
        ) as usize)
    };
    let u32_at = |i: usize| -> Result<usize, WeightsError> {
        Ok(u32::from_le_bytes(
            b.get(i..i + 4)
                .ok_or(WeightsError::Format)?
                .try_into()
                .unwrap(),
        ) as usize)
    };
    // The end of central directory record, searched from the end (a comment may follow).
    let eocd = (0..b.len().saturating_sub(21))
        .rev()
        .take(65_557)
        .find(|&i| b[i..].starts_with(b"PK\x05\x06"))
        .ok_or(WeightsError::Format)?;
    let count = u16_at(eocd + 10)?;
    let mut at = u32_at(eocd + 16)?;
    let mut out = HashMap::new();
    for _ in 0..count {
        if !b.get(at..).is_some_and(|r| r.starts_with(b"PK\x01\x02")) {
            return Err(WeightsError::Format);
        }
        let method = u16_at(at + 10)?;
        let size = u32_at(at + 20)?;
        let (name_len, extra_len, comment_len) =
            (u16_at(at + 28)?, u16_at(at + 30)?, u16_at(at + 32)?);
        let local = u32_at(at + 42)?;
        let name = std::str::from_utf8(
            b.get(at + 46..at + 46 + name_len)
                .ok_or(WeightsError::Format)?,
        )
        .map_err(|_| WeightsError::Format)?
        .to_owned();
        at += 46 + name_len + extra_len + comment_len;
        if method != 0 {
            return Err(WeightsError::Format);
        }
        let data = local + 30 + u16_at(local + 26)? + u16_at(local + 28)?;
        out.insert(name, b.get(data..data + size).ok_or(WeightsError::Format)?);
    }
    Ok(out)
}

/// A value of the pickle machine (only what a checkpoint holds).
#[derive(Clone, Debug)]
enum Value {
    Int(i64),
    Str(String),
    Bool,
    Tuple(Vec<Value>),
    Global(String),
    /// A storage: its key in `data/`.
    Storage(String),
    /// A tensor: storage key, offset, shape, strides.
    Tensor(String, usize, Vec<usize>, Vec<usize>),
    Dict(Vec<(Value, Value)>),
    Mark,
    None,
}

/// How many values `v` holds (a value too big to be a checkpoint's is refused: clones
/// of the memo could otherwise grow without end).
fn size(v: &Value) -> usize {
    match v {
        Value::Tuple(items) => 1 + items.iter().map(size).sum::<usize>(),
        Value::Dict(items) => 1 + items.iter().map(|(k, v)| size(k) + size(v)).sum::<usize>(),
        _ => 1,
    }
}

const MAX_VALUE: usize = 100_000;

fn as_usizes(v: &Value) -> Result<Vec<usize>, WeightsError> {
    match v {
        Value::Tuple(items) => items
            .iter()
            .map(|i| match i {
                Value::Int(n) if *n >= 0 => Ok(*n as usize),
                _ => Err(WeightsError::Format),
            })
            .collect(),
        _ => Err(WeightsError::Format),
    }
}

/// Runs the pickle `p` (protocol 2, the opcodes `torch.save` writes for a state dict).
fn unpickle(p: &[u8]) -> Result<Value, WeightsError> {
    let bad = || WeightsError::Format;
    let mut stack: Vec<Value> = Vec::new();
    let mut memo: HashMap<usize, Value> = HashMap::new();
    let mut i = 0;
    let byte = |i: usize| p.get(i).copied().ok_or(WeightsError::Format);
    let le = |i: usize, n: usize| -> Result<usize, WeightsError> {
        let s = p.get(i..i + n).ok_or(WeightsError::Format)?;
        Ok(s.iter()
            .rev()
            .fold(0usize, |a, b| (a << 8) | usize::from(*b)))
    };
    let pop_mark = |stack: &mut Vec<Value>| -> Result<Vec<Value>, WeightsError> {
        let at = stack
            .iter()
            .rposition(|v| matches!(v, Value::Mark))
            .ok_or(WeightsError::Format)?;
        let items = stack.split_off(at + 1);
        stack.pop();
        Ok(items)
    };
    loop {
        let op = byte(i)?;
        i += 1;
        match op {
            0x80 => i += 1,                           // PROTO
            b'}' => stack.push(Value::Dict(vec![])),  // EMPTY_DICT
            b')' => stack.push(Value::Tuple(vec![])), // EMPTY_TUPLE
            b'(' => stack.push(Value::Mark),          // MARK
            b'q' => {
                // BINPUT
                let v = stack.last().ok_or_else(bad)?;
                if size(v) > MAX_VALUE {
                    return Err(bad());
                }
                memo.insert(usize::from(byte(i)?), v.clone());
                i += 1;
            }
            b'r' => {
                // LONG_BINPUT
                let v = stack.last().ok_or_else(bad)?;
                if size(v) > MAX_VALUE {
                    return Err(bad());
                }
                memo.insert(le(i, 4)?, v.clone());
                i += 4;
            }
            b'h' => {
                // BINGET
                stack.push(memo.get(&usize::from(byte(i)?)).ok_or_else(bad)?.clone());
                i += 1;
            }
            b'j' => {
                // LONG_BINGET
                stack.push(memo.get(&le(i, 4)?).ok_or_else(bad)?.clone());
                i += 4;
            }
            b'K' => {
                stack.push(Value::Int(i64::from(byte(i)?)));
                i += 1;
            }
            b'M' => {
                stack.push(Value::Int(le(i, 2)? as i64));
                i += 2;
            }
            b'J' => {
                stack.push(Value::Int(i64::from(le(i, 4)? as u32 as i32)));
                i += 4;
            }
            b'X' => {
                // BINUNICODE
                let n = le(i, 4)?;
                let s = std::str::from_utf8(p.get(i + 4..i + 4 + n).ok_or_else(bad)?)
                    .map_err(|_| bad())?;
                stack.push(Value::Str(s.to_owned()));
                i += 4 + n;
            }
            b'c' => {
                // GLOBAL "module\nname\n"
                let end1 = p[i..].iter().position(|&c| c == b'\n').ok_or_else(bad)? + i;
                let end2 = p[end1 + 1..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .ok_or_else(bad)?
                    + end1
                    + 1;
                let module = std::str::from_utf8(&p[i..end1]).map_err(|_| bad())?;
                let name = std::str::from_utf8(&p[end1 + 1..end2]).map_err(|_| bad())?;
                stack.push(Value::Global(format!("{module}.{name}")));
                i = end2 + 1;
            }
            0x88 => stack.push(Value::Bool), // NEWTRUE
            0x89 => stack.push(Value::Bool), // NEWFALSE
            b'N' => stack.push(Value::None),
            b't' => {
                let items = pop_mark(&mut stack)?;
                stack.push(Value::Tuple(items));
            }
            0x85..=0x87 => {
                // TUPLE1/2/3
                let n = usize::from(op - 0x84);
                if stack.len() < n {
                    return Err(bad());
                }
                let items = stack.split_off(stack.len() - n);
                stack.push(Value::Tuple(items));
            }
            b'Q' => {
                // BINPERSID: ('storage', FloatStorage, key, location, numel)
                let Value::Tuple(pid) = stack.pop().ok_or_else(bad)? else {
                    return Err(bad());
                };
                match pid.as_slice() {
                    [Value::Str(kind), Value::Global(t), Value::Str(key), ..]
                        if kind == "storage" && t == "torch.FloatStorage" =>
                    {
                        stack.push(Value::Storage(key.clone()));
                    }
                    _ => return Err(bad()),
                }
            }
            b'R' => {
                // REDUCE: only the constructors of a state dict.
                let Value::Tuple(args) = stack.pop().ok_or_else(bad)? else {
                    return Err(bad());
                };
                let Value::Global(f) = stack.pop().ok_or_else(bad)? else {
                    return Err(bad());
                };
                match f.as_str() {
                    "torch._utils._rebuild_tensor_v2" => match args.as_slice() {
                        [Value::Storage(key), Value::Int(offset), shape, stride, ..]
                            if *offset >= 0 =>
                        {
                            stack.push(Value::Tensor(
                                key.clone(),
                                *offset as usize,
                                as_usizes(shape)?,
                                as_usizes(stride)?,
                            ));
                        }
                        _ => return Err(bad()),
                    },
                    "torch._utils._rebuild_parameter" => {
                        stack.push(args.into_iter().next().ok_or_else(bad)?);
                    }
                    "collections.OrderedDict" => stack.push(Value::Dict(vec![])),
                    _ => return Err(bad()),
                }
            }
            b'u' => {
                // SETITEMS
                let items = pop_mark(&mut stack)?;
                let Some(Value::Dict(d)) = stack.last_mut() else {
                    return Err(bad());
                };
                for kv in items.chunks(2) {
                    if let [k, v] = kv {
                        d.push((k.clone(), v.clone()));
                    }
                }
            }
            b's' => {
                // SETITEM
                let v = stack.pop().ok_or_else(bad)?;
                let k = stack.pop().ok_or_else(bad)?;
                let Some(Value::Dict(d)) = stack.last_mut() else {
                    return Err(bad());
                };
                d.push((k, v));
            }
            b'.' => return stack.pop().ok_or_else(bad),
            _ => return Err(bad()),
        }
    }
}

/// The tensors of a checkpoint: name → shape and f32 data.
pub fn read(bytes: &[u8]) -> Result<HashMap<String, Tensor>, WeightsError> {
    let members = zip_members(bytes)?;
    let (pkl_name, pkl) = members
        .iter()
        .find(|(n, _)| n.ends_with("/data.pkl"))
        .ok_or(WeightsError::Format)?;
    let root = pkl_name.trim_end_matches("data.pkl");
    let Value::Dict(mut items) = unpickle(pkl)? else {
        return Err(WeightsError::Format);
    };
    // A checkpoint may wrap the state dict: {"model_state_dict": {...}, ...}.
    if let Some((_, Value::Dict(inner))) = items
        .iter()
        .find(|(k, _)| matches!(k, Value::Str(s) if s == "model_state_dict"))
    {
        items = inner.clone();
    }
    let mut out = HashMap::new();
    for (k, v) in items {
        let (Value::Str(name), Value::Tensor(key, offset, shape, stride)) = (k, v) else {
            continue;
        };
        let n = shape
            .iter()
            .try_fold(1usize, |a, &d| a.checked_mul(d))
            .ok_or(WeightsError::Format)?;
        // Contiguous, row-major: what a saved state dict holds.
        let mut want = vec![1usize; shape.len()];
        for d in (0..shape.len().saturating_sub(1)).rev() {
            want[d] = want[d + 1].saturating_mul(shape[d + 1]);
        }
        if n > 1 && stride != want {
            return Err(WeightsError::Format);
        }
        let raw = members
            .get(&format!("{root}data/{key}"))
            .ok_or(WeightsError::Format)?
            .get(
                offset.checked_mul(4).ok_or(WeightsError::Format)?
                    ..offset
                        .checked_add(n)
                        .and_then(|e| e.checked_mul(4))
                        .ok_or(WeightsError::Format)?,
            )
            .ok_or(WeightsError::Format)?;
        let data = raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        out.insert(name.replace("smolgen", "gab"), Tensor { shape, data });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_what_is_not_a_checkpoint() {
        assert!(read(b"").is_err());
        assert!(read(b"PK\x05\x06garbage").is_err());
        assert!(read(&[0u8; 100]).is_err());
        // A pickle that calls anything but the state-dict constructors is refused.
        assert!(
            unpickle(b"\x80\x02cos\nsystem\nq\x00X\x02\x00\x00\x00lsq\x01\x85q\x02R.").is_err()
        );
        assert!(unpickle(b"\x80\x02}q\x00.").is_ok());
    }
}
