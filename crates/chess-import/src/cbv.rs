//! ChessBase archives (`.cbv`): a container of the files of one database, each
//! compressed in blocks with an LZ77-style scheme and an optional Huffman stage.
//!
//! Layout (little-endian), as implemented by uncbv (GPL-3.0-or-later, Antoni
//! Boucher), rewritten here without unsafe code and with bounds checks:
//!
//! - header: `08 00`, file count (u16), size of a directory entry (u8), 3 bytes;
//! - directory: per file, a zero-terminated name in a 132-byte field, the
//!   compressed and the decompressed size (i32), padded to the entry size;
//! - data: per file, blocks of `size (u16), unknown (u16), flag (u8), payload`,
//!   where flag bit 1 = Huffman-coded payload, bit 0 = LZ-compressed payload.
//!
//! Password-protected archives (`.cbz`, DES) are not supported.

use crate::ImportError;

/// One file of an archive, decompressed.
#[derive(Clone, Debug)]
pub struct Member {
    /// Name as stored (Windows-1252), `\` replaced by `/`.
    pub name: String,
    pub data: Vec<u8>,
}

const NAME_FIELD: usize = 132;
/// Upper bound for one decompressed member (a corrupt size must not exhaust memory).
const MAX_MEMBER: usize = 1 << 30;

/// True if `bytes` start like a CBV archive.
pub fn looks_like_cbv(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes[0] == 0x08 && bytes[1] == 0x00 && bytes[4] as usize >= NAME_FIELD + 8
}

/// Unpacks every member of the archive.
pub fn unpack(bytes: &[u8]) -> Result<Vec<Member>, ImportError> {
    let corrupt = |what: &str| ImportError::Corrupt(format!("CBV archive: {what}"));
    if !looks_like_cbv(bytes) {
        return Err(corrupt("not a ChessBase archive"));
    }
    let count = usize::from(u16::from_le_bytes([bytes[2], bytes[3]]));
    let entry = usize::from(bytes[4]);
    let mut pos = 8;
    let mut directory = Vec::with_capacity(count);
    for _ in 0..count {
        let e = bytes
            .get(pos..pos + entry)
            .ok_or_else(|| corrupt("truncated directory"))?;
        let name_bytes = &e[..NAME_FIELD];
        let end = name_bytes
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(NAME_FIELD);
        let (name, _, _) = encoding_rs::WINDOWS_1252.decode(&name_bytes[..end]);
        let compressed = i32::from_le_bytes([e[132], e[133], e[134], e[135]]);
        let decompressed = i32::from_le_bytes([e[136], e[137], e[138], e[139]]);
        let (Ok(compressed), Ok(decompressed)) =
            (usize::try_from(compressed), usize::try_from(decompressed))
        else {
            return Err(corrupt("negative size"));
        };
        if decompressed > MAX_MEMBER {
            return Err(corrupt("member too large"));
        }
        directory.push((name.replace('\\', "/"), compressed, decompressed));
        pos += entry;
    }
    let mut members = Vec::with_capacity(count);
    for (name, compressed, decompressed) in directory {
        let data = bytes
            .get(pos..pos + compressed)
            .ok_or_else(|| corrupt("truncated data"))?;
        pos += compressed;
        let out = unpack_member(data, decompressed)
            .map_err(|e| ImportError::Corrupt(format!("CBV archive, {name}: {e}")))?;
        members.push(Member { name, data: out });
    }
    Ok(members)
}

fn unpack_member(mut data: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(expected);
    while !data.is_empty() {
        if data.len() < 4 {
            return Err("truncated block header".into());
        }
        let size = usize::from(u16::from_le_bytes([data[0], data[1]]));
        let block = data.get(4..4 + size).ok_or("truncated block")?;
        data = &data[4 + size..];
        let (&flag, payload) = block.split_first().ok_or("empty block")?;
        if flag > 3 {
            return Err(format!("unknown block flag {flag}"));
        }
        let decoded;
        let payload = if flag & 0b10 != 0 {
            decoded = huffman(payload)?;
            &decoded[..]
        } else {
            payload
        };
        if flag & 0b01 != 0 {
            lz_decompress(payload, &mut out)?;
        } else {
            out.extend_from_slice(payload);
        }
        if out.len() > MAX_MEMBER {
            return Err("member too large".into());
        }
    }
    if out.len() != expected {
        return Err(format!(
            "size {} instead of the declared {expected}",
            out.len()
        ));
    }
    Ok(out)
}

/// LZ77 with run-length codes. Groups of 16 items are preceded by a 16-bit mask
/// (bit 15 first); a set bit marks a coded item, a clear bit a literal byte.
fn lz_decompress(mut input: &[u8], out: &mut Vec<u8>) -> Result<(), String> {
    // The block's output starts a new window: references never reach into earlier blocks.
    let base = out.len();
    let take = |input: &mut &[u8], n: usize| -> Result<Vec<u8>, String> {
        let s = input.get(..n).ok_or("truncated LZ data")?.to_vec();
        *input = &input[n..];
        Ok(s)
    };
    'outer: while !input.is_empty() {
        let m = take(&mut input, 2)?;
        let mut mask = u16::from_le_bytes([m[0], m[1]]);
        for _ in 0..16 {
            if input.is_empty() {
                break 'outer;
            }
            if mask & 0x8000 == 0 {
                out.push(input[0]);
                input = &input[1..];
            } else {
                let b = take(&mut input, 1)?[0];
                let (high, low) = (usize::from(b >> 4), usize::from(b & 0x0f));
                match high {
                    0 => {
                        // Short run: low + 3 copies of the next byte.
                        let v = take(&mut input, 1)?[0];
                        out.extend(std::iter::repeat_n(v, low + 3));
                    }
                    1 => {
                        // Long run: low + (next byte << 4) + 0x13 copies.
                        let x = take(&mut input, 2)?;
                        let n = low + (usize::from(x[0]) << 4) + 0x13;
                        out.extend(std::iter::repeat_n(x[1], n));
                    }
                    _ => {
                        // Back reference: offset from the next byte and `low`; the length
                        // is `high`, or a following byte + 0x10 when `high` is 2.
                        let x = take(&mut input, 1)?[0];
                        let offset = (usize::from(x) << 4) + low + 3;
                        let len = if high == 2 {
                            usize::from(take(&mut input, 1)?[0]) + 0x10
                        } else {
                            high
                        };
                        if offset > out.len() - base {
                            return Err("LZ reference before the start of the block".into());
                        }
                        let start = out.len() - offset;
                        // Byte by byte: the copy may overlap what it produces.
                        for i in 0..len {
                            let v = out[start + i];
                            out.push(v);
                        }
                    }
                }
            }
            mask <<= 1;
        }
    }
    Ok(())
}

/// Huffman stage: decompressed size (u16, big-endian), then a bit stream (most
/// significant bit first) with, for each of the 256 byte values, a 4-bit code
/// length and the code itself, followed by the coded data.
fn huffman(payload: &[u8]) -> Result<Vec<u8>, String> {
    let size = usize::from(u16::from_be_bytes([
        *payload.first().ok_or("truncated Huffman block")?,
        *payload.get(1).ok_or("truncated Huffman block")?,
    ]));
    let mut bits = Bits {
        data: &payload[2..],
        pos: 0,
    };
    // codes[len] maps a code of that length to its byte value.
    let mut codes: Vec<std::collections::HashMap<u16, u8>> = vec![Default::default(); 16];
    for value in 0..=255u8 {
        let len = bits.read(4).ok_or("truncated Huffman table")? as usize;
        if len > 0 {
            let code = bits.read(len).ok_or("truncated Huffman table")?;
            codes[len].insert(code, value);
        }
    }
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        let (mut code, mut len) = (0u16, 0usize);
        loop {
            code = (code << 1) | bits.read(1).ok_or("truncated Huffman data")?;
            len += 1;
            if len >= 16 {
                return Err("invalid Huffman code".into());
            }
            if let Some(v) = codes[len].get(&code) {
                out.push(*v);
                break;
            }
        }
    }
    Ok(out)
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: usize) -> Option<u16> {
        let mut v = 0u16;
        for _ in 0..n {
            let byte = *self.data.get(self.pos / 8)?;
            let bit = (byte >> (7 - self.pos % 8)) & 1;
            v = (v << 1) | u16::from(bit);
            self.pos += 1;
        }
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lz_runs_and_overlapping_references() {
        // mask: literal 'a', literal 'b', run of 4 'x', reference (offset 3, len 3)
        // Items: 'a' (0), 'b' (0), coded 0x01 'x' (1), coded 0x30 0x00 (1).
        let mask: u16 = 0b0011_0000_0000_0000;
        let mut input = mask.to_le_bytes().to_vec();
        input.extend_from_slice(b"ab");
        input.extend_from_slice(&[0x01, b'x']);
        input.extend_from_slice(&[0x30, 0x00]);
        let mut out = Vec::new();
        lz_decompress(&input, &mut out).expect("valid");
        assert_eq!(out, b"abxxxxxxx");
    }

    #[test]
    fn rejects_garbage() {
        assert!(unpack(b"PK\x03\x04").is_err());
        let mut fake = vec![0x08, 0x00, 0x01, 0x00, 0x95, 0, 0, 0];
        let mut entry = b"a.cbh".to_vec();
        entry.resize(NAME_FIELD, 0);
        entry.extend_from_slice(&100i32.to_le_bytes());
        entry.extend_from_slice(&10i32.to_le_bytes());
        entry.resize(0x95, 0);
        fake.extend(entry);
        assert!(unpack(&fake).is_err(), "member data past the end");
        fake.truncate(60);
        assert!(unpack(&fake).is_err(), "truncated directory");
    }
}
