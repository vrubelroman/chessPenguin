//! ChessBase databases (`.cbh` and companion files) converted to PGN text.
//!
//! The layout follows the public format description of the Morphy project
//! (`format/v1` in github.com/Yarin78/morphy), used as a source of facts; no code is taken
//! from it. Only reading is supported, and the source files are never written.

mod annotations;
mod entities;
mod headers;
mod moves;
mod text;
mod writer;

use std::collections::HashMap;
use std::path::Path;

use chess_core::TextEncoding;

use crate::ImportError;

/// The files of one database, by lower-case extension (`cbh`, `cbg`, …).
#[derive(Clone, Debug, Default)]
pub struct Files {
    pub name: String,
    files: HashMap<String, Vec<u8>>,
}

impl Files {
    /// Reads `<dir>/<stem>.<ext>` for every extension the reader uses.
    pub fn from_dir(cbh: &Path) -> Result<Files, ImportError> {
        let dir = cbh.parent().unwrap_or(Path::new("."));
        let stem = cbh
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut files = Files {
            name: stem.clone(),
            files: HashMap::new(),
        };
        let entries = std::fs::read_dir(dir).map_err(|source| ImportError::Io {
            path: dir.to_owned(),
            source,
        })?;
        for e in entries.flatten() {
            let p = e.path();
            let (Some(s), Some(ext)) = (p.file_stem(), p.extension()) else {
                continue;
            };
            let ext = ext.to_string_lossy().to_ascii_lowercase();
            if s.to_string_lossy() == stem && USED.contains(&ext.as_str()) {
                let bytes = std::fs::read(&p).map_err(|source| ImportError::Io {
                    path: p.clone(),
                    source,
                })?;
                files.files.insert(ext, bytes);
            }
        }
        Ok(files)
    }

    /// Picks the files of the database whose `.cbh` member is `cbh_name` from archive
    /// members (`name`, bytes).
    pub fn from_members<'a>(
        cbh_name: &str,
        members: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) -> Files {
        let stem = stem_of(cbh_name).to_owned();
        let mut files = Files {
            name: stem.rsplit('/').next().unwrap_or(&stem).to_owned(),
            files: HashMap::new(),
        };
        for (name, data) in members {
            if let Some(ext) = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())
                && stem_of(name) == stem
                && USED.contains(&ext.as_str())
            {
                files.files.insert(ext, data.to_vec());
            }
        }
        files
    }

    fn get(&self, ext: &str) -> Option<&[u8]> {
        self.files.get(ext).map(Vec::as_slice)
    }

    fn required(&self, ext: &str) -> Result<&[u8], ImportError> {
        self.get(ext).ok_or_else(|| {
            ImportError::Corrupt(format!(
                "ChessBase database {}: the .{ext} file is missing",
                self.name
            ))
        })
    }
}

/// Files the reader uses; the others (search boosters, settings, media) are ignored.
const USED: [&str; 6] = ["cbh", "cbg", "cba", "cbp", "cbt", "cbc"];

fn stem_of(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

/// Result of converting one database.
#[derive(Clone, Debug, Default)]
pub struct Conversion {
    /// UTF-8 PGN text, games and guiding texts in game id order.
    pub pgn: String,
    pub games: usize,
    pub texts: usize,
    /// Single-byte code page of the database's texts (UTF-8 texts are recognized
    /// individually).
    pub encoding: Option<TextEncoding>,
    /// Problems worth showing to the user, one line each.
    pub notes: Vec<String>,
}

/// Converts a database to PGN. Games that cannot be decoded completely are kept up
/// to the last readable move and reported in the notes.
pub fn convert(files: &Files) -> Result<Conversion, ImportError> {
    let cbh = files.required("cbh")?;
    let cbg = files.required("cbg")?;
    let cba = files.get("cba").unwrap_or(&[]);
    let records = headers::read(cbh)?;
    let players = entities::read(files.get("cbp"), entities::Kind::Player);
    let tournaments = entities::read(files.get("cbt"), entities::Kind::Tournament);
    let annotators = entities::read(files.get("cbc"), entities::Kind::Annotator);

    // One code page for the whole database, detected from all its single-byte texts.
    let mut sample = Vec::new();
    for e in [&players, &tournaments, &annotators] {
        e.sample(&mut sample);
    }
    annotations::sample_texts(cba, &records, &mut sample);
    let encoding = text::detect(&sample);
    let decoder = text::Decoder::new(encoding);

    let mut out = Conversion {
        encoding: Some(encoding),
        ..Conversion::default()
    };
    let mut problems = Problems::default();
    for (id, rec) in records.iter().enumerate() {
        let game_id = id + 1;
        match rec {
            headers::Record::Game(h) => {
                let tags = writer::tags(h, &players, &tournaments, &annotators, &decoder);
                let decoded = moves::decode(cbg, h.moves_offset);
                let (tree, error) = match decoded {
                    Ok(t) => (t, None),
                    Err(moves::DecodeError { partial, reason }) => (partial, Some(reason)),
                };
                if let Some(reason) = &error {
                    problems.add(game_id, &tags, reason);
                }
                let notes = if h.annotations_offset > 0 {
                    annotations::read(cba, h.annotations_offset, game_id, &decoder)
                } else {
                    Ok(annotations::Annotations::default())
                };
                let notes = notes.unwrap_or_else(|reason| {
                    problems.add(game_id, &tags, &reason);
                    annotations::Annotations::default()
                });
                writer::game(&mut out.pgn, &tags, &tree, &notes, h.result_tag());
                out.games += 1;
            }
            headers::Record::Text(t) => match text::guiding_text(cbg, t.text_offset, &decoder) {
                Ok(body) => {
                    let tags =
                        writer::text_tags(t, &body.title, &tournaments, &annotators, &decoder);
                    writer::text_game(&mut out.pgn, &tags, &body.text);
                    out.texts += 1;
                }
                Err(reason) => problems.add(game_id, &[], &reason),
            },
            headers::Record::Deleted => {}
        }
    }
    out.notes = problems.into_notes();
    Ok(out)
}

/// Collects per-game problems; reports each kind once with the number of games.
#[derive(Default)]
struct Problems {
    by_reason: Vec<(String, Vec<String>)>,
}

impl Problems {
    fn add(&mut self, game_id: usize, tags: &[(String, String)], reason: &str) {
        let tag = |k: &str| {
            tags.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or("?")
        };
        let who = if tags.is_empty() {
            format!("#{game_id}")
        } else {
            format!("#{game_id} {} – {}", tag("White"), tag("Black"))
        };
        match self.by_reason.iter_mut().find(|(r, _)| r == reason) {
            Some((_, games)) => games.push(who),
            None => self.by_reason.push((reason.to_owned(), vec![who])),
        }
    }

    fn into_notes(self) -> Vec<String> {
        self.by_reason
            .into_iter()
            .map(|(reason, games)| {
                let shown: Vec<&str> = games.iter().take(5).map(String::as_str).collect();
                let more = if games.len() > 5 {
                    format!(" (+{})", games.len() - 5)
                } else {
                    String::new()
                };
                format!("{reason}: {}{more}", shown.join("; "))
            })
            .collect()
    }
}

/// Big-endian unsigned integer of `n` bytes at `at`.
fn be(bytes: &[u8], at: usize, n: usize) -> Option<u32> {
    let s = bytes.get(at..at + n)?;
    Some(s.iter().fold(0u32, |acc, b| (acc << 8) | u32::from(*b)))
}

/// Little-endian signed 32-bit integer at `at`.
fn le_i32(bytes: &[u8], at: usize) -> Option<i32> {
    let s = bytes.get(at..at + 4)?;
    Some(i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
