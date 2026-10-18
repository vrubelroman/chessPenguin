//! `.cbp` players, `.cbt` tournaments, `.cbc` annotators: fixed-size records after a
//! small little-endian header; an entity id is the record's index.

use super::le_i32;
use super::text::Decoder;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Player,
    Tournament,
    Annotator,
}

/// Raw string fields of every record (decoded later, once the code page is known).
#[derive(Clone, Debug, Default)]
pub struct Entities {
    kind: Option<Kind>,
    records: Vec<Vec<Vec<u8>>>,
    /// Tournament start dates (packed), by id.
    dates: Vec<u32>,
}

/// Node of the tree (9 bytes) before the data of each record.
const NODE: usize = 9;

pub fn read(bytes: Option<&[u8]>, kind: Kind) -> Entities {
    let mut out = Entities {
        kind: Some(kind),
        ..Entities::default()
    };
    let Some(bytes) = bytes else { return out };
    let (Some(count), Some(data_size), Some(extra)) =
        (le_i32(bytes, 0), le_i32(bytes, 0x0c), le_i32(bytes, 0x18))
    else {
        return out;
    };
    let header = 28 + usize::try_from(extra).unwrap_or(0).min(4);
    let Ok(record) = usize::try_from(data_size).map(|d| d + NODE) else {
        return out;
    };
    // Fields: (offset in data, length).
    let fields: &[(usize, usize)] = match kind {
        Kind::Player => &[(0x00, 30), (0x1e, 20)],
        Kind::Tournament => &[(0x00, 40), (0x28, 30)],
        Kind::Annotator => &[(0x00, 45)],
    };
    for i in 0..usize::try_from(count).unwrap_or(0) {
        let Some(rec) = bytes.get(header + i * record..header + (i + 1) * record) else {
            break;
        };
        let data = &rec[NODE..];
        out.records.push(
            fields
                .iter()
                .map(|&(at, len)| {
                    let f = data.get(at..at + len).unwrap_or(&[]);
                    let end = f.iter().position(|b| *b == 0).unwrap_or(f.len());
                    f[..end].to_vec()
                })
                .collect(),
        );
        if kind == Kind::Tournament {
            out.dates.push(
                le_i32(data, 0x46)
                    .and_then(|d| u32::try_from(d).ok())
                    .unwrap_or(0),
            );
        }
    }
    out
}

impl Entities {
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn sample(&self, buf: &mut Vec<u8>) {
        for rec in &self.records {
            for f in rec {
                super::text::add_sample(buf, f);
            }
        }
    }

    fn field(&self, id: u32, i: usize, d: &Decoder) -> Option<String> {
        let f = self.records.get(id as usize)?.get(i)?;
        let s = d.decode_name(f).trim().to_owned();
        (!s.is_empty()).then_some(s)
    }

    /// "Last, First" for a player; the title of a tournament; an annotator's name.
    pub fn name(&self, id: u32, d: &Decoder) -> Option<String> {
        let first = self.field(id, 0, d);
        if self.kind == Some(Kind::Player) {
            return match (first, self.field(id, 1, d)) {
                (Some(last), Some(given)) => Some(format!("{last}, {given}")),
                (last, given) => last.or(given),
            };
        }
        first
    }

    /// Tournament place.
    pub fn place(&self, id: u32, d: &Decoder) -> Option<String> {
        self.field(id, 1, d)
    }

    pub fn date(&self, id: u32) -> u32 {
        self.dates.get(id as usize).copied().unwrap_or(0)
    }
}
