//! `.cba`: the annotations of a game, each attached to a move by its position in the
//! stored move order (−1 = the game as a whole). Big-endian.

use std::collections::BTreeMap;

use super::be;
use super::headers::Record;
use super::text::Decoder;

/// What can be shown in PGN. Other annotation types (training, game quotations,
/// pictures, clocks, …) are skipped.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Before(String),
    After(String),
    Nags(Vec<u8>),
    /// (colour letter, square index a1 = 0)
    Squares(Vec<(char, u8)>),
    Arrows(Vec<(char, u8, u8)>),
    /// Time spent on the move: h, m, s.
    TimeSpent(u8, u8, u8),
}

#[derive(Clone, Debug, Default)]
pub struct Annotations {
    pub at: BTreeMap<i32, Vec<Item>>,
}

struct Raw<'a> {
    position: i32,
    kind: u8,
    data: &'a [u8],
}

const RECORD_HEADER: usize = 14;

fn parse(cba: &[u8], offset: u32) -> Result<(u32, Vec<Raw<'_>>), String> {
    let at = offset as usize;
    let (Some(game), Some(size)) = (be(cba, at, 3), be(cba, at + 10, 4)) else {
        return Err("annotations outside the .cba file".into());
    };
    let record = cba
        .get(at..at + size as usize)
        .filter(|r| r.len() >= RECORD_HEADER)
        .ok_or("annotations outside the .cba file")?;
    let mut items = Vec::new();
    let mut i = RECORD_HEADER;
    while i + 6 <= record.len() {
        let pos = be(record, i, 3).unwrap_or(0);
        // Sign-extend the 24-bit position.
        let position = ((pos << 8) as i32) >> 8;
        let kind = record[i + 3];
        let len = be(record, i + 4, 2).unwrap_or(0) as usize;
        if len < 6 || i + len > record.len() {
            return Err("damaged annotation record".into());
        }
        items.push(Raw {
            position,
            kind,
            data: &record[i + 6..i + len],
        });
        i += len;
    }
    Ok((game, items))
}

/// Adds the text bytes of every game's annotations to `buf` (for code page detection).
pub fn sample_texts(cba: &[u8], records: &[Record], buf: &mut Vec<u8>) {
    for r in records {
        if let Record::Game(h) = r
            && h.annotations_offset > 0
        {
            sample_at(cba, h.annotations_offset, buf);
        }
    }
}

/// Adds the text bytes of one annotation record (starting at byte 0) to `buf`.
pub fn sample_record(record: &[u8], buf: &mut Vec<u8>) {
    sample_at(record, 0, buf);
}

fn sample_at(cba: &[u8], offset: u32, buf: &mut Vec<u8>) {
    if let Ok((_, items)) = parse(cba, offset) {
        for it in items {
            if matches!(it.kind, 0x02 | 0x82) && it.data.len() > 2 {
                super::text::add_sample(buf, &it.data[2..]);
            }
        }
    }
}

pub fn read(cba: &[u8], offset: u32, game_id: usize, d: &Decoder) -> Result<Annotations, String> {
    let (game, items) = parse(cba, offset)?;
    if game as usize != game_id {
        return Err("annotations belong to another game".into());
    }
    let mut out = Annotations::default();
    for it in items {
        let item = match it.kind {
            0x02 | 0x82 => {
                let text = d
                    .decode(it.data.get(2..).unwrap_or(&[]))
                    .replace(super::text::OBJECT, " ")
                    .replace("\r\n", "\n")
                    .replace('\r', "\n");
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                if it.kind == 0x02 {
                    Item::After(text.to_owned())
                } else {
                    Item::Before(text.to_owned())
                }
            }
            0x03 => {
                let nags: Vec<u8> = it.data.iter().copied().filter(|n| *n != 0).collect();
                if nags.is_empty() {
                    continue;
                }
                Item::Nags(nags)
            }
            0x04 => Item::Squares(
                it.data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .filter_map(|c| Some((colour(c[0])?, c[1].checked_sub(1)?)))
                    .filter(|(_, sq)| *sq < 64)
                    .collect(),
            ),
            0x05 => Item::Arrows(
                it.data
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .filter_map(|c| {
                        Some((colour(c[0])?, c[1].checked_sub(1)?, c[2].checked_sub(1)?))
                    })
                    .filter(|(_, a, b)| *a < 64 && *b < 64)
                    .collect(),
            ),
            0x07 if it.data.len() >= 3 => Item::TimeSpent(it.data[0], it.data[1], it.data[2]),
            _ => continue,
        };
        out.at.entry(it.position).or_default().push(item);
    }
    Ok(out)
}

/// ChessBase colours 2 green, 3 yellow, 4 red as PGN `%csl`/`%cal` letters.
fn colour(c: u8) -> Option<char> {
    match c {
        2 => Some('G'),
        3 => Some('Y'),
        4 => Some('R'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::TextEncoding;

    fn ann(position: i32, kind: u8, data: &[u8]) -> Vec<u8> {
        let mut v = (position as u32).to_be_bytes()[1..].to_vec();
        v.push(kind);
        v.extend_from_slice(&((data.len() + 6) as u16).to_be_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn reads_texts_symbols_and_graphics() {
        let mut body = Vec::new();
        body.extend(ann(-1, 0x02, b"\x00\x00Intro"));
        body.extend(ann(0, 0x03, &[1, 0, 0]));
        body.extend(ann(0, 0x02, &[0, 0, 0xd0, 0xe0, 0xe7, 0x9e]));
        body.extend(ann(1, 0x04, &[2, 1, 4, 64]));
        body.extend(ann(1, 0x05, &[3, 13, 29, 9, 1, 2]));
        let size = (RECORD_HEADER + body.len()) as u32;
        let mut cba = vec![0u8; 26];
        let offset = cba.len() as u32;
        cba.extend_from_slice(&[0, 0, 7, 1, 0, 0x0e, 0x0e, 0, 0, 6]);
        cba.extend_from_slice(&size.to_be_bytes());
        cba.extend(body);
        let d = Decoder::new(TextEncoding::Windows1251);
        let a = read(&cba, offset, 7, &d).expect("record");
        assert_eq!(a.at[&-1], vec![Item::After("Intro".into())]);
        assert_eq!(
            a.at[&0],
            vec![Item::Nags(vec![1]), Item::After("Раз".into())],
            "cp1251 text, diagram marker removed"
        );
        assert_eq!(
            a.at[&1],
            vec![
                Item::Squares(vec![('G', 0), ('R', 63)]),
                Item::Arrows(vec![('Y', 12, 28)]),
            ],
            "unknown colour 9 dropped"
        );
        assert!(read(&cba, offset, 8, &d).is_err(), "wrong game id");
    }
}
