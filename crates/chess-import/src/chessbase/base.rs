//! A ChessBase database read in place, for databases of any size (Mega Database:
//! 11.7 million games). Only the `.cbh` records and the player, tournament and
//! annotator lists are kept in memory; the moves and annotations of a game are read
//! from `.cbg` / `.cba` when the game is opened. Nothing is ever written.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::entities::{self, Entities, Kind};
use super::headers::{self, GameHeader, Record};
use super::text::{self, Decoder};
use super::{annotations, be, booster, moves, openings, writer};
use crate::ImportError;

const RECORD: usize = 46;
/// Game flags of a `.cbh` record (offset 0x27).
const FLAG_VARIATIONS: u32 = 0x2;
const FLAG_COMMENTS: u32 = 0x4;

/// What a record of the database is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordKind {
    Game,
    Text,
    Deleted,
}

/// A ChessBase database opened for reading.
pub struct Base {
    path: PathBuf,
    /// The `.cbh` records after the header record, `RECORD` bytes each.
    cbh: Vec<u8>,
    records: usize,
    games: usize,
    texts: usize,
    players: Names,
    tournaments: Names,
    annotators: Entities,
    tournament_entities: Entities,
    player_entities: Entities,
    decoder: Decoder,
    cbg_path: PathBuf,
    cbg: Mutex<File>,
    /// Records in the `.cbh` file, trailing empty ones included (the `.cbb` booster
    /// counts them).
    file_records: usize,
    /// The `.cbb` search booster, when present and consistent (checked on first use).
    cbb: OnceLock<Option<PathBuf>>,
    /// Where our own booster is kept when `.cbb` cannot be used (set by the
    /// application), and that booster once found or built.
    booster_dir: OnceLock<PathBuf>,
    own_booster: OnceLock<PathBuf>,
    /// The opening index, once loaded (`openings.rs`; kept in the same directory).
    openings: Mutex<Option<Arc<openings::Index>>>,
    /// Held by [`Base::prepare`]: two callers (the search board, the reference panel)
    /// never build the same files at once; the second finds them done.
    preparing: Mutex<()>,
    cba: Option<Mutex<File>>,
    /// Titles of guiding texts read so far (they are stored with the text).
    text_titles: Mutex<HashMap<usize, String>>,
    /// Remarks about the database as a whole, one line each.
    notes: Vec<String>,
}

/// Decoded names of a list, folded copies for searching (see [`fold`]), the
/// alphabetical rank of each (for sorting by name) and the number of games of each.
pub(crate) struct Names {
    shown: Vec<String>,
    folded: Vec<String>,
    rank: Vec<u32>,
    games: Vec<u32>,
}

impl Names {
    pub(crate) fn new(shown: Vec<String>) -> Names {
        let folded: Vec<String> = shown.iter().map(|s| fold(s)).collect();
        let mut order: Vec<u32> = (0..folded.len() as u32).collect();
        order.sort_by(|&a, &b| folded[a as usize].cmp(&folded[b as usize]));
        let mut rank = vec![0; folded.len()];
        for (r, id) in order.into_iter().enumerate() {
            // An empty name has no value: it sorts last, like a missing rating.
            rank[id as usize] = if folded[id as usize].trim().is_empty() {
                u32::MAX
            } else {
                r as u32
            };
        }
        let games = vec![0; shown.len()];
        Names {
            shown,
            folded,
            rank,
            games,
        }
    }

    pub(crate) fn count(&mut self, id: u32) {
        if let Some(n) = self.games.get_mut(id as usize) {
            *n += 1;
        }
    }

    pub(crate) fn get(&self, id: u32) -> &str {
        self.shown.get(id as usize).map_or("", String::as_str)
    }

    pub(crate) fn rank(&self, id: u32) -> u32 {
        self.rank.get(id as usize).copied().unwrap_or(u32::MAX)
    }

    /// Ids whose name contains every word of `query`, as a lookup table.
    pub(crate) fn matching(&self, query: &str) -> Vec<bool> {
        let words = words(query);
        self.folded
            .iter()
            .map(|n| words.iter().all(|w| n.contains(w.as_str())))
            .collect()
    }

    /// Up to `limit` names containing every word of `query`: names starting with
    /// the first word first, then by number of games. Also the number of matches.
    pub(crate) fn suggest(&self, query: &str, limit: usize) -> (Vec<Suggestion>, usize) {
        let words = words(query);
        if words.is_empty() {
            return (Vec::new(), 0);
        }
        let mut hits: Vec<(bool, u32, u32)> = self
            .folded
            .iter()
            .enumerate()
            .filter(|(_, n)| words.iter().all(|w| n.contains(w.as_str())))
            .map(|(id, n)| {
                let id = id as u32;
                (
                    !n.starts_with(words[0].as_str()),
                    id,
                    self.games[id as usize],
                )
            })
            .collect();
        let total = hits.len();
        hits.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(b.2.cmp(&a.2))
                .then(self.rank(a.1).cmp(&self.rank(b.1)))
        });
        let list = hits
            .into_iter()
            .take(limit)
            .map(|(_, id, games)| Suggestion {
                id,
                name: self.get(id).to_owned(),
                games,
            })
            .collect();
        (list, total)
    }
}

/// A name offered while typing a filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub id: u32,
    pub name: String,
    pub games: u32,
}

/// Lower case without diacritics ("Hübner" → "hubner", "Ł" → "l"), for searching.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        let base = match c {
            'à'..='å' | 'ā' | 'ă' | 'ą' => "a",
            'æ' => "ae",
            'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
            'ď' | 'đ' | 'ð' => "d",
            'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
            // «Артём» is typed «Артем».
            'ё' => "е",
            'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
            'ĥ' | 'ħ' => "h",
            'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
            'ĵ' => "j",
            'ķ' => "k",
            'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
            'ñ' | 'ń' | 'ņ' | 'ň' => "n",
            'ò'..='ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
            'œ' => "oe",
            'ŕ' | 'ŗ' | 'ř' => "r",
            'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => "s",
            'ß' => "ss",
            'ţ' | 'ť' | 'ŧ' | 'ț' => "t",
            'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
            'ŵ' => "w",
            'ý' | 'ÿ' | 'ŷ' => "y",
            'ź' | 'ż' | 'ž' => "z",
            'þ' => "th",
            _ => {
                out.push(c);
                continue;
            }
        };
        out.push_str(base);
    }
    out
}

/// Folded words of a search query; commas separate words too ("Carlsen, M").
pub(crate) fn words(query: &str) -> Vec<String> {
    fold(query)
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// One row of the game table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub kind: Option<RecordKind>,
    pub white: String,
    pub black: String,
    pub white_elo: u16,
    pub black_elo: u16,
    /// "1-0", "0-1", "1/2-1/2" or "*".
    pub result: &'static str,
    pub eco: Option<String>,
    /// A Chess960 game (its moves cannot be read).
    pub chess960: bool,
    pub event: String,
    pub site: String,
    /// PGN date ("1997.05.??").
    pub date: String,
    pub round: String,
    pub annotator: String,
    /// Moves of the main line (255: 255 or more; 0: none).
    pub moves: u8,
    pub has_variations: bool,
    pub has_comments: bool,
    /// The title of a guiding text.
    pub title: String,
}

/// Fields of the table a game list can be sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SortKey {
    Number,
    White,
    WhiteElo,
    Black,
    BlackElo,
    Result,
    Moves,
    Eco,
    Event,
    Date,
    /// Average rating of the two players (only when both are known); equal averages
    /// by date.
    AverageElo,
}

/// Which side a player filter applies to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Side {
    #[default]
    Any,
    White,
    Black,
}

/// A game result. With a player filter, [`Filter::player_relative`] makes
/// `WhiteWins` mean "the player won" and `BlackWins` "the player lost".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    WhiteWins,
    Draw,
    BlackWins,
    Unknown,
}

/// Whose rating [`Filter::elo_min`] applies to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EloRule {
    #[default]
    Both,
    Either,
    Average,
    /// The opponent of the player given in the filter.
    Opponent,
}

/// A name filter: words to look for, or one entity picked from the suggestions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct NameFilter {
    pub text: String,
    pub exact: Option<u32>,
}

impl NameFilter {
    pub fn is_set(&self) -> bool {
        self.exact.is_some() || !words(&self.text).is_empty()
    }
}

/// A game filter; empty fields match everything. Guiding texts have no players,
/// results, ratings or dates: any such condition leaves them out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Filter {
    pub player: NameFilter,
    pub side: Side,
    pub opponent: NameFilter,
    /// The tournament's title or place.
    pub event: NameFilter,
    pub annotator: String,
    pub year_from: Option<u16>,
    pub year_to: Option<u16>,
    pub elo_min: Option<u16>,
    pub elo_rule: EloRule,
    /// ECO ranges as numbers 0 (A00) … 499 (E99), inclusive (see [`parse_eco`]).
    pub eco: Vec<(u16, u16)>,
    pub outcomes: Vec<Outcome>,
    pub player_relative: bool,
    /// Main-line moves, inclusive.
    pub moves_from: Option<u16>,
    pub moves_to: Option<u16>,
    pub with_comments: bool,
    pub with_variations: bool,
    /// Leave guiding texts out.
    pub games_only: bool,
    /// The quick search: every word in some field of the game — a player, the
    /// tournament (title, place, year), the annotator, the ECO code, the date.
    pub text: String,
}

/// ECO ranges of a filter: codes ("B90"), prefixes ("B9" = B90-B99, "B" = B00-B99)
/// and ranges ("C60-C99"), separated by commas. `None` if the text is not valid.
pub fn parse_eco(text: &str) -> Option<Vec<(u16, u16)>> {
    let one = |s: &str| -> Option<(u16, u16)> {
        let s = s.trim().to_ascii_uppercase();
        let mut chars = s.chars();
        let letter = chars.next()?;
        if !('A'..='E').contains(&letter) {
            return None;
        }
        let base = (letter as u16 - 'A' as u16) * 100;
        let digits: String = chars.collect();
        if !digits.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        match digits.len() {
            0 => Some((base, base + 99)),
            1 => {
                let d = digits.parse::<u16>().ok()? * 10;
                Some((base + d, base + d + 9))
            }
            2 => {
                let d = digits.parse::<u16>().ok()?;
                Some((base + d, base + d))
            }
            _ => None,
        }
    };
    let mut out = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let part = part.replace(['–', '—'], "-");
        let range = match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (one(a)?, one(b)?);
                (a.0 <= b.1).then_some((a.0, b.1))?
            }
            None => one(&part)?,
        };
        out.push(range);
    }
    (!out.is_empty()).then_some(out)
}

impl Base {
    /// Opens the database whose `.cbh` file is `cbh`. `progress` gets (bytes read,
    /// bytes total) of the record file.
    pub fn open(
        cbh: &Path,
        progress: &mut dyn FnMut(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<Base, ImportError> {
        let io = |path: &Path| {
            let path = path.to_owned();
            move |source| ImportError::Io { path, source }
        };
        let companion = |ext: &str| find_companion(cbh, ext);
        let cbh_bytes = read_with_progress(cbh, progress, cancel)?;
        if cbh_bytes.len() < RECORD || be(&cbh_bytes, 3, 2) != Some(RECORD as u32) {
            return Err(ImportError::Corrupt(
                "ChessBase database: the .cbh file has an unknown layout".into(),
            ));
        }
        let truncated = (cbh_bytes.len() % RECORD) != 0;
        let mut body = cbh_bytes;
        body.drain(..RECORD);
        let file_records = body.len() / RECORD;
        // All-zero records lie past the last game; a damaged record in between is
        // treated as deleted (see `kind`), so the records after it stay readable.
        let records = body
            .as_chunks::<RECORD>()
            .0
            .iter()
            .rposition(|r| r[0] & 1 != 0)
            .map_or(0, |last| last + 1);
        body.truncate(records * RECORD);
        body.shrink_to_fit();

        let cbg_path = companion("cbg").ok_or_else(|| missing(cbh, "cbg"))?;
        let cbg = File::open(&cbg_path).map_err(io(&cbg_path))?;
        let cba = companion("cba")
            .map(|p| File::open(&p).map_err(io(&p)))
            .transpose()?;
        let read_list = |ext: &str| -> Result<Option<Vec<u8>>, ImportError> {
            companion(ext)
                .map(|p| std::fs::read(&p).map_err(io(&p)))
                .transpose()
        };
        let player_entities = entities::read(read_list("cbp")?.as_deref(), Kind::Player);
        let tournament_entities = entities::read(read_list("cbt")?.as_deref(), Kind::Tournament);
        let annotators = entities::read(read_list("cbc")?.as_deref(), Kind::Annotator);
        if cancel.load(Ordering::Relaxed) {
            return Err(ImportError::Cancelled);
        }

        let mut base = Base {
            path: cbh.to_owned(),
            cbh: body,
            records,
            games: 0,
            texts: 0,
            players: Names::new(Vec::new()),
            tournaments: Names::new(Vec::new()),
            annotators,
            tournament_entities,
            player_entities,
            decoder: Decoder::new(chess_core::TextEncoding::Windows1252),
            cbg_path,
            cbg: Mutex::new(cbg),
            file_records,
            cbb: OnceLock::new(),
            booster_dir: OnceLock::new(),
            own_booster: OnceLock::new(),
            openings: Mutex::new(None),
            preparing: Mutex::new(()),
            cba: cba.map(Mutex::new),
            text_titles: Mutex::new(HashMap::new()),
            notes: Vec::new(),
        };
        if truncated {
            base.notes
                .push("the .cbh file ends inside a record; the last record is left out".into());
        }
        base.decoder = Decoder::new(base.detect_encoding());
        let d = &base.decoder;
        let players = (0..base.player_entities.len() as u32)
            .map(|id| base.player_entities.name(id, d).unwrap_or_default())
            .collect();
        let tournaments = (0..base.tournament_entities.len() as u32)
            .map(|id| {
                let title = base.tournament_entities.name(id, d).unwrap_or_default();
                let mut name = match base.tournament_entities.place(id, d) {
                    Some(place) if !title.contains(&place) => format!("{title} ({place})"),
                    _ => title,
                };
                // Tournaments of the same name in different years: tell them apart.
                let year = (base.tournament_entities.date(id) >> 9) & 0xfff;
                if year > 0 && !name.contains(&year.to_string()) {
                    name = format!("{name} {year}");
                }
                name.trim().to_owned()
            })
            .collect();
        base.players = Names::new(players);
        base.tournaments = Names::new(tournaments);
        // Counts: records by kind, games by player and tournament (for suggestions).
        let (mut games, mut texts, mut chess960, mut damaged) = (0, 0, 0, 0);
        for r in base.cbh.as_chunks::<RECORD>().0 {
            let u = |at, n| be(r.as_slice(), at, n).unwrap_or(0);
            if r[0] & 1 == 0 {
                damaged += 1;
                continue;
            }
            if r[0] & 0x80 != 0 {
                continue;
            }
            if r[0] & 0x02 != 0 {
                texts += 1;
                continue;
            }
            games += 1;
            if u(0x23, 2) >= 64576 {
                chess960 += 1;
            }
            base.players.count(u(0x09, 3));
            base.players.count(u(0x0c, 3));
            base.tournaments.count(u(0x0f, 3));
        }
        base.games = games;
        base.texts = texts;
        if chess960 > 0 {
            base.notes.push(format!(
                "Chess960 games are not supported: {chess960} games are shown without moves"
            ));
        }
        if damaged > 0 {
            base.notes.push(format!(
                "{damaged} damaged records in the .cbh file are left out"
            ));
        }
        Ok(base)
    }

    /// Code page of the single-byte texts, from the names and the comments of the
    /// first annotated games.
    fn detect_encoding(&self) -> chess_core::TextEncoding {
        let mut sample = Vec::new();
        for e in [
            &self.player_entities,
            &self.tournament_entities,
            &self.annotators,
        ] {
            e.sample(&mut sample);
        }
        let mut annotated = 0;
        for i in 0..self.records {
            if annotated >= 500 || sample.len() > 4 << 20 {
                break;
            }
            let offset = be(self.record(i), 5, 4).unwrap_or(0);
            if self.kind(i) == RecordKind::Game
                && offset > 0
                && let Some(bytes) = self.read_annotations(offset)
            {
                annotations::sample_record(&bytes, &mut sample);
                annotated += 1;
            }
        }
        text::detect(&sample)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The `.cbg` file, for readers that open their own handle (position search).
    pub(super) fn cbg_path(&self) -> &Path {
        &self.cbg_path
    }

    /// Makes position searches run without the `.cbb` booster (for measurements).
    #[cfg(test)]
    pub(super) fn without_booster(&self) {
        let _ = self.cbb.set(None);
    }

    /// Offset of the record's moves in `.cbg` (games and texts).
    pub(super) fn moves_offset(&self, index: usize) -> u32 {
        be(self.record(index), 1, 4).unwrap_or(0)
    }

    /// The search booster to use: `.cbb` if it can be trusted (its record count is
    /// that of the `.cbh` file plus one, it holds that many 52-byte records and it is
    /// not older than `.cbg`: a program that does not maintain it may have changed the
    /// games since), else our own one (see [`Base::build_booster`]).
    pub(super) fn cbb_path(&self) -> Option<&Path> {
        self.cbb
            .get_or_init(|| {
                let path = find_companion(&self.path, "cbb")?;
                let cbg_time = std::fs::metadata(&self.cbg_path).ok()?.modified().ok()?;
                let fresh = std::fs::metadata(&path).ok()?.modified().ok()? >= cbg_time;
                (fresh && booster_consistent(&path, self.file_records)).then_some(path)
            })
            .as_deref()
            .or_else(|| self.own_booster.get().map(PathBuf::as_path))
    }

    /// Records in the `.cbh` file, trailing empty ones included.
    pub(super) fn file_records(&self) -> usize {
        self.file_records
    }

    /// Tells where our own booster lives (a cache directory), and takes the one
    /// built earlier for this database if it is there.
    pub fn set_booster_dir(&self, dir: &Path) {
        let _ = self.booster_dir.set(dir.to_owned());
        if let Some(p) = self.own_booster_path()
            && booster_consistent(&p, self.file_records)
        {
            let _ = self.own_booster.set(p);
        }
    }

    /// Where the files of this database go in the cache directory, without the
    /// extension: named after the database and the size and date of its games, so a
    /// changed database gets new ones.
    fn cache_stem(&self) -> Option<PathBuf> {
        let dir = self.booster_dir.get()?;
        let path = std::fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
        let meta = std::fs::metadata(&self.cbg_path).ok()?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in path
            .to_string_lossy()
            .bytes()
            .chain(meta.len().to_le_bytes())
            .chain(modified.to_le_bytes())
            .chain((self.file_records as u64).to_le_bytes())
        {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        Some(dir.join(format!("{h:016x}")))
    }

    fn own_booster_path(&self) -> Option<PathBuf> {
        self.cache_stem().map(|p| p.with_extension("cbb"))
    }

    fn openings_path(&self) -> Option<PathBuf> {
        self.cache_stem().map(|p| p.with_extension("cpo"))
    }

    /// The opening index, when it is loaded.
    pub fn openings(&self) -> Option<Arc<openings::Index>> {
        self.openings.lock().ok()?.clone()
    }

    /// Where the opening index stands.
    pub fn openings_status(&self) -> OpeningsStatus {
        if let Some(index) = self.openings() {
            return OpeningsStatus::Loaded(index.memory());
        }
        let Some(path) = self.openings_path() else {
            return OpeningsStatus::NoCache;
        };
        match openings::Index::file_size(&path) {
            Some(bytes) => OpeningsStatus::OnDisk {
                bytes,
                room: room_for(bytes),
            },
            None => OpeningsStatus::Missing {
                room: room_for(self.records as u64 * openings::BYTES_PER_GAME),
            },
        }
    }

    /// Something is left to build or load for fast position searches
    /// ([`Base::prepare`]).
    pub fn prepare_wanted(&self) -> bool {
        self.booster_wanted()
            || matches!(
                self.openings_status(),
                OpeningsStatus::Missing { room: true } | OpeningsStatus::OnDisk { room: true, .. }
            )
    }

    /// The work [`Base::prepare`] has to do, in the units of its `progress`: the
    /// records of the pass, and as many again for sorting the opening index.
    pub fn prepare_work(&self) -> usize {
        let pass = self.booster_wanted() || self.openings_to_build();
        let sort = self.openings_to_build();
        self.records * (usize::from(pass) + usize::from(sort))
    }

    fn openings_to_build(&self) -> bool {
        matches!(
            self.openings_status(),
            OpeningsStatus::Missing { room: true }
        )
    }

    /// Builds what is missing for fast position searches — our booster when `.cbb`
    /// cannot be used, the opening index when the machine has room for it — in one
    /// pass over the games (`progress` counts the records done, and again while the
    /// index is sorted), then loads the index. `Ok(false)` when cancelled.
    pub fn prepare(&self, progress: &AtomicUsize, cancel: &AtomicBool) -> std::io::Result<bool> {
        let _one_at_a_time = self
            .preparing
            .lock()
            .map_err(|_| std::io::Error::other("prepare lock"))?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let Some(stem) = self.cache_stem() else {
            return Err(std::io::Error::other("no cache directory"));
        };
        let booster_path = self.booster_wanted().then(|| stem.with_extension("cbb"));
        let openings_path = stem.with_extension("cpo");
        let build_openings = matches!(
            self.openings_status(),
            OpeningsStatus::Missing { room: true }
        );
        if booster_path.is_some() || build_openings {
            let built = booster::build(
                self,
                booster_path.as_deref(),
                build_openings.then_some(openings_path.as_path()),
                progress,
                cancel,
            )?;
            if !built {
                return Ok(false);
            }
            if let Some(p) = booster_path {
                let _ = self.own_booster.set(p);
            }
        }
        if self.openings().is_none()
            && let Some(bytes) = openings::Index::file_size(&openings_path)
            && room_for(bytes)
        {
            match openings::Index::load(&openings_path, self.records) {
                Some(index) => {
                    if let Ok(mut o) = self.openings.lock() {
                        *o = Some(Arc::new(index));
                    }
                }
                // Not an index for this database (an older layout): built anew next time.
                None => {
                    let _ = std::fs::remove_file(&openings_path);
                }
            }
        }
        Ok(true)
    }

    /// Position searches have a booster (ChessBase's or ours).
    pub fn booster_ready(&self) -> bool {
        self.cbb_path().is_some()
    }

    /// A booster of our own would help and can be built ([`Base::set_booster_dir`]).
    pub fn booster_wanted(&self) -> bool {
        !self.booster_ready() && self.booster_dir.get().is_some()
    }

    /// Builds our own booster (one pass over all the games; `progress` counts the
    /// records done) and uses it from then on. `Ok(false)` when cancelled.
    pub fn build_booster(
        &self,
        progress: &AtomicUsize,
        cancel: &AtomicBool,
    ) -> std::io::Result<bool> {
        let Some(path) = self.own_booster_path() else {
            return Err(std::io::Error::other("no booster directory"));
        };
        if !booster::build(self, Some(&path), None, progress, cancel)? {
            return Ok(false);
        }
        let _ = self.own_booster.set(path);
        Ok(true)
    }

    /// Number of records (games, texts and deleted records); a record's index is its
    /// game number minus one.
    pub fn records(&self) -> usize {
        self.records
    }

    pub fn games(&self) -> usize {
        self.games
    }

    pub fn texts(&self) -> usize {
        self.texts
    }

    pub fn encoding(&self) -> chess_core::TextEncoding {
        self.decoder.single_byte()
    }

    /// The raw 46-byte record (also a stable identity of the game).
    pub fn record(&self, index: usize) -> &[u8] {
        self.cbh
            .get(index * RECORD..(index + 1) * RECORD)
            .unwrap_or(&[])
    }

    /// Remarks about the database as a whole (Chess960 games, damaged records).
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    pub fn kind(&self, index: usize) -> RecordKind {
        match self.record(index).first() {
            Some(k) if k & 0x80 != 0 || k & 1 == 0 => RecordKind::Deleted,
            Some(k) if k & 0x02 != 0 => RecordKind::Text,
            Some(_) => RecordKind::Game,
            None => RecordKind::Deleted,
        }
    }

    fn header(&self, index: usize) -> Option<Record> {
        // `headers::read` expects the file header first.
        let mut buf = [0u8; 2 * RECORD];
        buf[3..5].copy_from_slice(&(RECORD as u16).to_be_bytes());
        buf[RECORD..].copy_from_slice(self.record(index).get(..RECORD)?);
        headers::read(&buf).ok()?.into_iter().next()
    }

    pub fn row(&self, index: usize) -> Row {
        match self.header(index) {
            Some(Record::Game(h)) => self.game_row(&h),
            Some(Record::Text(t)) => Row {
                kind: Some(RecordKind::Text),
                event: self.tournaments.get(t.tournament).to_owned(),
                annotator: self
                    .annotators
                    .name(t.annotator, &self.decoder)
                    .unwrap_or_default(),
                round: headers::round_tag(t.round, t.subround),
                result: "*",
                date: "????.??.??".into(),
                title: self.text_title(index, t.text_offset),
                ..Row::default()
            },
            _ => Row {
                kind: Some(RecordKind::Deleted),
                result: "*",
                ..Row::default()
            },
        }
    }

    fn game_row(&self, h: &GameHeader) -> Row {
        let d = &self.decoder;
        Row {
            kind: Some(RecordKind::Game),
            white: self.players.get(h.white).to_owned(),
            black: self.players.get(h.black).to_owned(),
            white_elo: h.white_elo,
            black_elo: h.black_elo,
            result: h.result_tag(),
            eco: h.eco_tag(),
            chess960: h.eco >= 64576,
            event: self
                .tournament_entities
                .name(h.tournament, d)
                .unwrap_or_default(),
            site: self
                .tournament_entities
                .place(h.tournament, d)
                .unwrap_or_default(),
            date: headers::date_tag(h.date),
            round: headers::round_tag(h.round, h.subround),
            annotator: self.annotators.name(h.annotator, d).unwrap_or_default(),
            moves: h.moves,
            has_variations: h.flags & FLAG_VARIATIONS != 0,
            has_comments: h.flags & FLAG_COMMENTS != 0,
            title: String::new(),
        }
    }

    fn text_title(&self, index: usize, offset: u32) -> String {
        if let Ok(cache) = self.text_titles.lock()
            && let Some(t) = cache.get(&index)
        {
            return t.clone();
        }
        let title = self
            .read_moves(offset)
            .and_then(|b| text::guiding_text(&b, 0, &self.decoder).ok())
            .map(|b| b.title)
            .unwrap_or_default();
        if let Ok(mut cache) = self.text_titles.lock() {
            cache.insert(index, title.clone());
        }
        title
    }

    /// The record as PGN text (UTF-8), and a problem when the game could only be read
    /// in part. `None` for a deleted record.
    pub fn pgn(&self, index: usize) -> Option<(String, Option<String>)> {
        let game_id = index + 1;
        let mut out = String::new();
        match self.header(index)? {
            Record::Game(h) => {
                let d = &self.decoder;
                let tags = writer::tags(
                    &h,
                    &self.player_entities,
                    &self.tournament_entities,
                    &self.annotators,
                    d,
                );
                let (tree, mut problem) = match self.read_moves(h.moves_offset) {
                    None => (
                        moves::Tree::start(),
                        Some("moves outside the .cbg file".to_owned()),
                    ),
                    Some(bytes) => match moves::decode(&bytes, 0) {
                        Ok(t) => (t, None),
                        Err(e) => (e.partial, Some(e.reason)),
                    },
                };
                let notes = if h.annotations_offset > 0 {
                    self.read_annotations(h.annotations_offset)
                        .ok_or_else(|| "annotations outside the .cba file".to_owned())
                        .and_then(|b| annotations::read(&b, 0, game_id, d))
                        .unwrap_or_else(|reason| {
                            problem.get_or_insert(reason);
                            annotations::Annotations::default()
                        })
                } else {
                    annotations::Annotations::default()
                };
                writer::game(&mut out, &tags, &tree, &notes, h.result_tag());
                Some((out, problem))
            }
            Record::Text(t) => {
                let d = &self.decoder;
                let body = self
                    .read_moves(t.text_offset)
                    .ok_or_else(|| "text outside the .cbg file".to_owned())
                    .and_then(|b| text::guiding_text(&b, 0, d));
                let (body, problem) = match body {
                    Ok(b) => (b, None),
                    Err(e) => (
                        text::Body {
                            title: String::new(),
                            text: String::new(),
                        },
                        Some(e),
                    ),
                };
                let tags = writer::text_tags(
                    &t,
                    &body.title,
                    &self.tournament_entities,
                    &self.annotators,
                    d,
                );
                writer::text_game(&mut out, &tags, &body.text);
                Some((out, problem))
            }
            Record::Deleted => None,
        }
    }

    /// The `.cbg` record at `offset` (its size is in bytes 1-3).
    fn read_moves(&self, offset: u32) -> Option<Vec<u8>> {
        let mut f = self.cbg.lock().ok()?;
        read_sized(&mut f, offset, 0, 1, 3)
    }

    /// The `.cba` record at `offset` (its size is in bytes 10-13).
    fn read_annotations(&self, offset: u32) -> Option<Vec<u8>> {
        let mut f = self.cba.as_ref()?.lock().ok()?;
        read_sized(&mut f, offset, 14, 10, 4)
    }

    /// Player names for a filter being typed, and the number of matches.
    pub fn suggest_players(&self, query: &str, limit: usize) -> (Vec<Suggestion>, usize) {
        self.players.suggest(query, limit)
    }

    /// Tournament names ("Title (Place)") for a filter being typed.
    pub fn suggest_events(&self, query: &str, limit: usize) -> (Vec<Suggestion>, usize) {
        self.tournaments.suggest(query, limit)
    }

    pub fn player_name(&self, id: u32) -> &str {
        self.players.get(id)
    }

    pub fn event_name(&self, id: u32) -> &str {
        self.tournaments.get(id)
    }

    /// Indices of the records that pass `filter`, sorted. `scanned` counts the
    /// records looked at so far (for a progress display). `None` when cancelled.
    pub fn query(
        &self,
        filter: &Filter,
        sort: SortKey,
        ascending: bool,
        scanned: &AtomicUsize,
        cancel: &AtomicBool,
    ) -> Option<Vec<u32>> {
        let player_ids = name_table(&self.players, &filter.player);
        let opponent_ids = name_table(&self.players, &filter.opponent);
        let event_ids = name_table(&self.tournaments, &filter.event);
        let annotator = words(&filter.annotator);
        let annotator_ids = (!annotator.is_empty()).then(|| {
            (0..self.annotators.len() as u32)
                .map(|id| {
                    let name = fold(&self.annotators.name(id, &self.decoder).unwrap_or_default());
                    annotator.iter().all(|w| name.contains(w.as_str()))
                })
                .collect::<Vec<bool>>()
        });
        let has = |ids: &Option<Vec<bool>>, id: u32| {
            ids.as_ref()
                .is_none_or(|t| t.get(id as usize).copied().unwrap_or(false))
        };
        // The quick search, word by word: the names it is in.
        let quick: Vec<QuickWord> = words(&filter.text)
            .into_iter()
            .map(|w| QuickWord {
                players: self.players.matching(&w),
                events: self.tournaments.matching(&w),
                annotators: (0..self.annotators.len() as u32)
                    .map(|id| {
                        fold(&self.annotators.name(id, &self.decoder).unwrap_or_default())
                            .contains(w.as_str())
                    })
                    .collect(),
                // A record's year has 12 bits and its ECO 500 values: tables, not text.
                years: (0..4096u32)
                    .map(|y| y > 0 && y.to_string().contains(w.as_str()))
                    .collect(),
                ecos: (0..500u16)
                    .map(|c| fold(&eco_text(c)).contains(w.as_str()))
                    .collect(),
            })
            .collect();
        let names = player_ids.is_some() || opponent_ids.is_some();
        let game_only_conditions = names
            || filter.year_from.is_some()
            || filter.year_to.is_some()
            || filter.elo_min.is_some()
            || !filter.eco.is_empty()
            || !filter.outcomes.is_empty()
            || filter.moves_from.is_some()
            || filter.moves_to.is_some()
            || filter.with_comments
            || filter.with_variations;
        let relative = filter.player_relative && player_ids.is_some();

        let mut out: Vec<u32> = Vec::new();
        for (i, r) in self.cbh.as_chunks::<RECORD>().0.iter().enumerate() {
            if i % 65536 == 0 {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                scanned.store(i, Ordering::Relaxed);
            }
            let u = |at, n| be(r.as_slice(), at, n).unwrap_or(0);
            let kind = r[0];
            if kind & 0x80 != 0 || kind & 1 == 0 {
                continue;
            }
            if kind & 0x02 != 0 {
                // A guiding text: only the tournament and the annotator apply.
                // The quick search looks in its tournament and annotator.
                let at = |t: &Vec<bool>, id: u32| t.get(id as usize).copied().unwrap_or(false);
                if !filter.games_only
                    && !game_only_conditions
                    && has(&event_ids, u(0x07, 3))
                    && has(&annotator_ids, u(0x0d, 3))
                    && quick
                        .iter()
                        .all(|q| at(&q.events, u(0x07, 3)) || at(&q.annotators, u(0x0d, 3)))
                {
                    out.push(i as u32);
                }
                continue;
            }
            let (white, black) = (u(0x09, 3), u(0x0c, 3));
            // Which side the player of the filter is on (both: either way round).
            let mut player_white = true;
            let mut player_black = true;
            if names {
                let as_white = has(&player_ids, white) && has(&opponent_ids, black);
                let as_black = has(&player_ids, black) && has(&opponent_ids, white);
                player_white = as_white && filter.side != Side::Black;
                player_black = as_black && filter.side != Side::White;
                if !player_white && !player_black {
                    continue;
                }
            }
            if !has(&event_ids, u(0x0f, 3)) || !has(&annotator_ids, u(0x12, 3)) {
                continue;
            }
            if filter.year_from.is_some() || filter.year_to.is_some() {
                let year = (u(0x18, 3) >> 9) & 0xfff;
                if year == 0
                    || filter.year_from.is_some_and(|y| year < u32::from(y))
                    || filter.year_to.is_some_and(|y| year > u32::from(y))
                {
                    continue;
                }
            }
            if let Some(min) = filter.elo_min.map(u32::from) {
                let (we, bl) = (u(0x1f, 2), u(0x21, 2));
                let ok = match filter.elo_rule {
                    EloRule::Both => we >= min && bl >= min,
                    EloRule::Either => we >= min || bl >= min,
                    EloRule::Average => we > 0 && bl > 0 && (we + bl) / 2 >= min,
                    EloRule::Opponent if player_ids.is_some() => {
                        (player_white && bl >= min) || (player_black && we >= min)
                    }
                    EloRule::Opponent => we >= min || bl >= min,
                };
                if !ok {
                    continue;
                }
            }
            if !filter.eco.is_empty() {
                let Some(code) = eco_number(u(0x23, 2) as u16) else {
                    continue;
                };
                if !filter.eco.iter().any(|&(a, b)| (a..=b).contains(&code)) {
                    continue;
                }
            }
            if !filter.outcomes.is_empty() {
                let board = match r[0x1b] {
                    0 | 4 => Outcome::BlackWins,
                    1 | 5 => Outcome::Draw,
                    2 | 6 => Outcome::WhiteWins,
                    _ => Outcome::Unknown,
                };
                let wanted = |o: Outcome| filter.outcomes.contains(&o);
                let ok = if relative && matches!(board, Outcome::WhiteWins | Outcome::BlackWins) {
                    // WhiteWins = the player won, BlackWins = the player lost.
                    let flipped = match board {
                        Outcome::WhiteWins => Outcome::BlackWins,
                        _ => Outcome::WhiteWins,
                    };
                    (player_white && wanted(board)) || (player_black && wanted(flipped))
                } else {
                    wanted(board)
                };
                if !ok {
                    continue;
                }
            }
            if filter.moves_from.is_some() || filter.moves_to.is_some() {
                let moves = u32::from(r[0x2d]);
                if filter.moves_from.is_some_and(|m| moves < u32::from(m))
                    || filter.moves_to.is_some_and(|m| moves > u32::from(m))
                {
                    continue;
                }
            }
            let flags = u(0x27, 4);
            if (filter.with_comments && flags & FLAG_COMMENTS == 0)
                || (filter.with_variations && flags & FLAG_VARIATIONS == 0)
            {
                continue;
            }
            if !quick.is_empty() {
                let year = (u(0x18, 3) >> 9) & 0xfff;
                let eco = eco_number(u(0x23, 2) as u16);
                let found = quick.iter().all(|q| {
                    let at = |t: &Vec<bool>, id: u32| t.get(id as usize).copied().unwrap_or(false);
                    at(&q.players, white)
                        || at(&q.players, black)
                        || at(&q.events, u(0x0f, 3))
                        || at(&q.annotators, u(0x12, 3))
                        || eco.is_some_and(|c| at(&q.ecos, u32::from(c)))
                        || at(&q.years, year)
                });
                if !found {
                    continue;
                }
            }
            out.push(i as u32);
        }
        scanned.store(self.records, Ordering::Relaxed);
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        self.sort_records(&mut out, sort, ascending);
        Some(out)
    }

    /// Sorts record indices (in record order on entry) by `sort`; records without a
    /// value (no rating, no date, a text) stay at the end in both directions.
    pub fn sort_records(&self, records: &mut Vec<u32>, sort: SortKey, ascending: bool) {
        if sort != SortKey::Number {
            let mut keyed: Vec<(u64, u32)> = records
                .iter()
                .map(|&i| (self.sort_key(i as usize, sort), i))
                .collect();
            keyed.sort_unstable();
            *records = keyed.into_iter().map(|(_, i)| i).collect();
        } else {
            records.sort_unstable();
        }
        if !ascending {
            let split = records
                .iter()
                .position(|&i| self.sort_key(i as usize, sort) == u64::MAX)
                .unwrap_or(records.len());
            records[..split].reverse();
        }
    }

    /// Sort value of a record; `u64::MAX` for no value (no rating, no date, a text).
    fn sort_key(&self, index: usize, sort: SortKey) -> u64 {
        let r = self.record(index);
        let u = |at, n| be(r, at, n).unwrap_or(0);
        let text = r.first().is_some_and(|k| k & 0x02 != 0);
        if text && !matches!(sort, SortKey::Number | SortKey::Event) {
            return u64::MAX;
        }
        let none_if_zero = |v: u32| if v == 0 { u64::MAX } else { u64::from(v) };
        match sort {
            SortKey::Number => index as u64,
            SortKey::White => self.players.rank(u(0x09, 3)).into(),
            SortKey::Black => self.players.rank(u(0x0c, 3)).into(),
            SortKey::WhiteElo => none_if_zero(u(0x1f, 2)),
            SortKey::BlackElo => none_if_zero(u(0x21, 2)),
            SortKey::Result => match r.get(0x1b) {
                Some(2 | 6) => 0,
                Some(1 | 5) => 1,
                Some(0 | 4) => 2,
                _ => u64::MAX,
            },
            SortKey::Moves => none_if_zero(u32::from(r.get(0x2d).copied().unwrap_or(0))),
            SortKey::Eco => eco_number(u(0x23, 2) as u16).map_or(u64::MAX, u64::from),
            SortKey::Event => self
                .tournaments
                .rank(if text { u(0x07, 3) } else { u(0x0f, 3) })
                .into(),
            SortKey::Date => none_if_zero(u(0x18, 3)),
            SortKey::AverageElo => {
                let (w, b) = (u(0x1f, 2), u(0x21, 2));
                if w == 0 || b == 0 {
                    u64::MAX
                } else {
                    // The average first, then the date.
                    (u64::from((w + b) / 2) << 32) | u64::from(u(0x18, 3))
                }
            }
        }
    }
}

/// A word of the quick search and the names it is in.
struct QuickWord {
    players: Vec<bool>,
    events: Vec<bool>,
    annotators: Vec<bool>,
    /// Years (0–4095) whose number holds the word.
    years: Vec<bool>,
    /// ECO codes (0 = A00 … 499 = E99) that hold it.
    ecos: Vec<bool>,
}

/// 190 → "B90".
pub(crate) fn eco_text(code: u16) -> String {
    format!("{}{:02}", char::from(b'A' + (code / 100) as u8), code % 100)
}

/// Ids matching a name filter (`None`: no condition).
pub(crate) fn name_table(names: &Names, f: &NameFilter) -> Option<Vec<bool>> {
    if let Some(id) = f.exact {
        let mut t = vec![false; names.shown.len()];
        if let Some(x) = t.get_mut(id as usize) {
            *x = true;
        }
        return Some(t);
    }
    (!words(&f.text).is_empty()).then(|| names.matching(&f.text))
}

/// Where the opening index of a database stands (see [`Base::openings_status`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpeningsStatus {
    /// In memory, this many bytes.
    Loaded(usize),
    /// Built, but not loaded; `room`: the machine has memory for it now.
    OnDisk { bytes: u64, room: bool },
    /// Not built; `room`: the machine has memory to build and hold it.
    Missing { room: bool },
    /// No cache directory was given.
    NoCache,
}

/// Whether `bytes` more can be held in memory with a gigabyte to spare. Unknown
/// memory counts as no room.
fn room_for(bytes: u64) -> bool {
    crate::memory::available().is_some_and(|free| free >= bytes + (1 << 30))
}

/// A booster file holds a record per `.cbh` record (plus its header) and says so.
fn booster_consistent(path: &Path, file_records: usize) -> bool {
    let Ok(mut f) = File::open(path) else {
        return false;
    };
    let mut head = [0u8; 4];
    if f.read_exact(&mut head).is_err() {
        return false;
    }
    let count = u32::from_be_bytes(head) as u64;
    count == file_records as u64 + 1 && f.metadata().is_ok_and(|m| m.len() >= 52 * count)
}

/// ECO code number 0 (A00) … 499 (E99) of a `.cbh` ECO field.
fn eco_number(eco: u16) -> Option<u16> {
    if eco == 0 || eco >= 64576 {
        return None;
    }
    (eco / 128).checked_sub(1).filter(|c| *c < 500)
}

/// Reads a record whose size is stored as a `size_len`-byte big-endian number at
/// `size_at`; `head` bytes are read first (at least enough to hold the size).
fn read_sized(
    f: &mut File,
    offset: u32,
    head: usize,
    size_at: usize,
    size_len: usize,
) -> Option<Vec<u8>> {
    let head = head.max(size_at + size_len);
    f.seek(SeekFrom::Start(u64::from(offset))).ok()?;
    let mut buf = vec![0u8; head];
    f.read_exact(&mut buf).ok()?;
    let size = be(&buf, size_at, size_len)? as usize;
    // Records hold at most a few megabytes; a larger size means damaged data.
    if size < head || size > 64 << 20 {
        return None;
    }
    buf.resize(size, 0);
    f.read_exact(&mut buf[head..]).ok()?;
    Some(buf)
}

/// `<dir>/<stem>.<ext>`, whatever the case of the extension.
fn find_companion(cbh: &Path, ext: &str) -> Option<PathBuf> {
    let dir = cbh.parent().unwrap_or(Path::new("."));
    let stem = cbh.file_stem()?;
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_stem() == Some(stem)
                && p.extension()
                    .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(ext))
        })
}

fn missing(cbh: &Path, ext: &str) -> ImportError {
    ImportError::Corrupt(format!(
        "ChessBase database {}: the .{ext} file is missing",
        cbh.file_stem().unwrap_or_default().to_string_lossy()
    ))
}

fn read_with_progress(
    path: &Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<Vec<u8>, ImportError> {
    let io = |source| ImportError::Io {
        path: path.to_owned(),
        source,
    };
    let mut f = File::open(path).map_err(io)?;
    let total = f.metadata().map_err(io)?.len();
    let mut out = Vec::with_capacity(total as usize);
    let mut chunk = vec![0u8; 8 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ImportError::Cancelled);
        }
        let n = f.read(&mut chunk).map_err(io)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..n]);
        progress(out.len() as u64, total);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eco_filters() {
        assert_eq!(parse_eco("B90"), Some(vec![(190, 190)]));
        assert_eq!(parse_eco("b9"), Some(vec![(190, 199)]));
        assert_eq!(parse_eco("E"), Some(vec![(400, 499)]));
        assert_eq!(parse_eco("C60-C99, A00"), Some(vec![(260, 299), (0, 0)]));
        assert_eq!(parse_eco("C6–D"), Some(vec![(260, 399)]));
        assert_eq!(parse_eco("C99-C60"), None);
        assert_eq!(parse_eco("F00"), None);
        assert_eq!(parse_eco("B901"), None);
        assert_eq!(parse_eco(""), None);
    }

    #[test]
    fn folding_and_words() {
        assert_eq!(fold("Hübner, Robert"), "hubner, robert");
        assert_eq!(fold("Łukasz Ćwikła"), "lukasz cwikla");
        assert_eq!(words("  Carlsen,Magnus "), vec!["carlsen", "magnus"]);
        let names = Names::new(vec![
            "Carlsen, Magnus".into(),
            "Carlsen, Henrik".into(),
            "Hübner, Robert".into(),
        ]);
        assert_eq!(names.matching("magnus carlsen"), vec![true, false, false]);
        assert_eq!(names.matching("hubner"), vec![false, false, true]);
        let (list, total) = names.suggest("carl", 1);
        assert_eq!((list.len(), total), (1, 2));
    }

    #[test]
    fn eco_numbers() {
        assert_eq!(eco_number((201 + 1) * 128 + 5), Some(201));
        assert_eq!(eco_number(0), None);
        assert_eq!(eco_number(64576), None);
    }
}
