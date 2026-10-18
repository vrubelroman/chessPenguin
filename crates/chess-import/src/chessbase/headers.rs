//! `.cbh`: one 46-byte record per game or guiding text (big-endian).

use super::be;
use crate::ImportError;

const RECORD: usize = 46;

#[derive(Clone, Debug)]
pub enum Record {
    Game(GameHeader),
    Text(TextHeader),
    Deleted,
}

#[derive(Clone, Debug, Default)]
pub struct GameHeader {
    pub moves_offset: u32,
    pub annotations_offset: u32,
    pub white: u32,
    pub black: u32,
    pub tournament: u32,
    pub annotator: u32,
    pub date: u32,
    pub result: u8,
    pub round: u8,
    pub subround: u8,
    pub white_elo: u16,
    pub black_elo: u16,
    pub eco: u16,
    /// Annotation flags (variations, comments, symbols, …).
    pub flags: u32,
    /// Main-line moves (255: 255 or more).
    pub moves: u8,
}

#[derive(Clone, Debug, Default)]
pub struct TextHeader {
    pub text_offset: u32,
    pub tournament: u32,
    pub annotator: u32,
    pub round: u8,
    pub subround: u8,
}

pub fn read(cbh: &[u8]) -> Result<Vec<Record>, ImportError> {
    if cbh.len() < RECORD || be(cbh, 3, 2) != Some(RECORD as u32) {
        return Err(ImportError::Corrupt(
            "ChessBase database: the .cbh file has an unknown layout".into(),
        ));
    }
    let mut out = Vec::new();
    for rec in cbh[RECORD..].as_chunks::<RECORD>().0 {
        let kind = rec[0];
        if kind & 1 == 0 {
            // All-zero records lie past the last game.
            break;
        }
        let u = |at, n| be(rec.as_slice(), at, n).unwrap_or(0);
        out.push(if kind & 0x80 != 0 {
            Record::Deleted
        } else if kind & 0x02 != 0 {
            Record::Text(TextHeader {
                text_offset: u(0x01, 4),
                tournament: u(0x07, 3),
                annotator: u(0x0d, 3),
                round: rec[0x10],
                subround: rec[0x11],
            })
        } else {
            Record::Game(GameHeader {
                moves_offset: u(0x01, 4),
                annotations_offset: u(0x05, 4),
                white: u(0x09, 3),
                black: u(0x0c, 3),
                tournament: u(0x0f, 3),
                annotator: u(0x12, 3),
                date: u(0x18, 3),
                result: rec[0x1b],
                round: rec[0x1d],
                subround: rec[0x1e],
                white_elo: u(0x1f, 2) as u16,
                black_elo: u(0x21, 2) as u16,
                eco: u(0x23, 2) as u16,
                flags: u(0x27, 4),
                moves: rec[0x2d],
            })
        });
    }
    Ok(out)
}

impl GameHeader {
    pub fn result_tag(&self) -> &'static str {
        match self.result {
            0 | 4 => "0-1",
            1 | 5 => "1/2-1/2",
            2 | 6 => "1-0",
            _ => "*",
        }
    }

    /// "C41" (sub-codes are ChessBase-specific and dropped); `None` for no ECO or Chess960.
    pub fn eco_tag(&self) -> Option<String> {
        if self.eco == 0 || self.eco >= 64576 {
            return None;
        }
        let code = usize::from(self.eco / 128).checked_sub(1)?;
        let letter = *b"ABCDE".get(code / 100)?;
        Some(format!("{}{:02}", letter as char, code % 100))
    }
}

/// "1858.??.??" from a packed date (day bits 0-4, month 5-8, year 9-20).
pub fn date_tag(packed: u32) -> String {
    let (day, month, year) = (packed & 31, (packed >> 5) & 15, (packed >> 9) & 0xfff);
    let part = |v: u32, w: usize| {
        if v == 0 {
            "?".repeat(w)
        } else {
            format!("{v:0w$}")
        }
    };
    format!("{}.{}.{}", part(year, 4), part(month, 2), part(day, 2))
}

pub fn round_tag(round: u8, subround: u8) -> String {
    match (round, subround) {
        (0, _) => "?".into(),
        (r, 0) => r.to_string(),
        (r, s) => format!("{r}.{s}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_eco_and_rounds() {
        assert_eq!(date_tag((1905 << 9) | (8 << 5) | 31), "1905.08.31");
        assert_eq!(date_tag(1907 << 9), "1907.??.??");
        assert_eq!(date_tag(0), "????.??.??");
        let h = |eco| GameHeader {
            eco,
            ..GameHeader::default()
        };
        assert_eq!(h((201 + 1) * 128 + 5).eco_tag().as_deref(), Some("C01"));
        assert_eq!(h(128).eco_tag().as_deref(), Some("A00"));
        assert_eq!(h(0).eco_tag(), None);
        assert_eq!(h(64576).eco_tag(), None);
        assert_eq!(round_tag(0, 0), "?");
        assert_eq!(round_tag(3, 0), "3");
        assert_eq!(round_tag(3, 2), "3.2");
    }
}
