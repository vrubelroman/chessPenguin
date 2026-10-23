//! The game list of a PGN database with the filters, sorting and name suggestions of a
//! ChessBase database ([`crate::chessbase::base`]): built from the tags read when the
//! file was opened, so a filter is a pass over a few arrays in memory. Fields a file
//! does not have (no ECO, no Annotator) simply match nothing when filtered on.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chess_core::pgn::GameEntry;

use crate::chessbase::base::{
    EloRule, Filter, Names, Outcome, RecordKind, Row, Side, SortKey, Suggestion, fold, name_table,
    words,
};

/// The tag the ChessBase converter gives a guiding text (its title is the `White`
/// tag): such a record is a text, not a game, as in the ChessBase database.
pub const TEXT_TAG: &str = "CbText";

/// A record of a PGN converted from ChessBase that was a guiding text.
pub fn is_text(e: &GameEntry) -> bool {
    e.tag(TEXT_TAG).is_some()
}

/// What a filter and a sort need of each game, one array per field.
pub struct Catalog {
    players: Names,
    events: Names,
    white: Vec<u32>,
    black: Vec<u32>,
    event: Vec<u32>,
    /// Folded `Annotator` tags.
    annotator: Vec<String>,
    /// `yyyymmdd` with unknown parts 0; 0 for no date.
    date: Vec<u32>,
    /// When the game was played, for sorting: `yyyymmdd` × 100 000 + seconds of the day
    /// (`UTCDate` and `UTCTime` when both are there, as online games have them).
    when: Vec<u64>,
    white_elo: Vec<u16>,
    black_elo: Vec<u16>,
    result: Vec<Outcome>,
    /// ECO as a number 0 (A00) … 499 (E99).
    eco: Vec<Option<u16>>,
    moves: Vec<u16>,
    comments: Vec<bool>,
    variations: Vec<bool>,
    /// Which fields at least one game has (an empty result says so, §9.8).
    filled: Filled,
    /// Most games have no result: a book, whose continuations count chapters.
    books: bool,
    /// Guiding texts (from a converted ChessBase database).
    text: Vec<bool>,
    texts: usize,
}

/// Fields at least one game of the database has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Filled {
    pub eco: bool,
    pub annotator: bool,
    pub elo: bool,
    pub year: bool,
}

/// "2026.10.18", "2026-10-18", "2026/10/18", "1997.05.??", "2010" → yyyymmdd (unknown
/// parts 0); 0 without a year.
pub fn date_number(date: &str) -> u32 {
    let mut parts = date.trim().split(['.', '-', '/']);
    let num = |s: Option<&str>, max: u32| -> u32 {
        s.filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|n| *n <= max)
            .unwrap_or(0)
    };
    let year = num(parts.next(), 9999);
    if !(1..=9999).contains(&year) {
        return 0;
    }
    year * 10000 + num(parts.next(), 12) * 100 + num(parts.next(), 31)
}

/// yyyymmdd → the date as the table shows it ("18.10.2026", "10.2026", "2026"), for the
/// quick search.
fn shown_date(d: u32) -> String {
    let (y, m, day) = (d / 10000, d / 100 % 100, d % 100);
    match (y, m, day) {
        (0, ..) => String::new(),
        (y, 0, _) => y.to_string(),
        (y, m, 0) => format!("{m:02}.{y}"),
        (y, m, day) => format!("{day:02}.{m:02}.{y}"),
    }
}

/// "13:05:09" → seconds of the day.
fn seconds(time: &str) -> Option<u32> {
    let mut parts = time.trim().split(':').map(|p| p.parse::<u32>().ok());
    let (h, m, s) = (
        parts.next()??,
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
    );
    (h < 24 && m < 60 && s < 61).then_some(h * 3600 + m * 60 + s)
}

/// "2026-10-18" → "2026.10.18": the PGN way, as the table shows dates.
fn pgn_date(date: &str) -> String {
    date.trim().replace(['-', '/'], ".")
}

/// "B90", "b90a", "B90 Sicilian" → 190 (A00 = 0 … E99 = 499): the first three
/// characters.
fn eco_of(tag: &str) -> Option<u16> {
    let t = tag.trim().as_bytes();
    let t = t.get(..3)?;
    if !(b'A'..=b'E').contains(&t[0].to_ascii_uppercase()) || !t[1..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    Some(
        u16::from(t[0].to_ascii_uppercase() - b'A') * 100
            + u16::from(t[1] - b'0') * 10
            + u16::from(t[2] - b'0'),
    )
}

fn outcome(result: Option<&str>) -> Outcome {
    match result.map(str::trim) {
        Some("1-0") => Outcome::WhiteWins,
        Some("0-1") => Outcome::BlackWins,
        Some("1/2-1/2" | "½-½") => Outcome::Draw,
        _ => Outcome::Unknown,
    }
}

/// A name the way a list shows it, spaces squeezed; none for "?", "-" and "" (PGN's
/// unknown).
fn known(name: Option<&str>) -> String {
    let n = name
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if n == "?" || n == "-" {
        String::new()
    } else {
        n
    }
}

/// When the game was played as yyyymmdd: `Date`, or `UTCDate` when `Date` has no year.
fn game_date(e: &GameEntry) -> u32 {
    let d = date_number(e.date().unwrap_or(""));
    if d / 10000 > 0 {
        d
    } else {
        date_number(e.tag("UTCDate").unwrap_or(""))
    }
}

/// The place of a game, from `Site`: not "?", not a link (Lichess and Chess.com put the
/// game's address there).
fn place(e: &GameEntry) -> String {
    let site = known(e.tag("Site"));
    if site.contains("://") {
        String::new()
    } else {
        site
    }
}

/// A tournament as ChessBase names it: "Title (place) year", the place and the year
/// only when the title does not have them already.
fn event_name(e: &GameEntry) -> String {
    let title = known(e.event());
    let place = place(e);
    let mut name = if !place.is_empty() && !title.contains(&place) {
        if title.is_empty() {
            place
        } else {
            format!("{title} ({place})")
        }
    } else {
        title
    };
    if name.is_empty() {
        return name;
    }
    let year = game_date(e) / 10000;
    if year > 0 && !name.contains(&year.to_string()) {
        name = format!("{name} {year}");
    }
    name
}

/// Ids of the distinct names, in first-seen order, and each game's id.
fn intern(values: impl Iterator<Item = String>) -> (Vec<String>, Vec<u32>) {
    let mut ids = std::collections::HashMap::new();
    let mut shown = Vec::new();
    let of = values
        .map(|v| {
            *ids.entry(v.clone()).or_insert_with(|| {
                shown.push(v);
                shown.len() as u32 - 1
            })
        })
        .collect();
    (shown, of)
}

impl Catalog {
    pub fn build(entries: &[GameEntry]) -> Catalog {
        // White and Black share one list: a player is the same whichever colour.
        // A text has a title, not players.
        let player = |e: &GameEntry, name: Option<&str>| {
            if is_text(e) {
                String::new()
            } else {
                known(name)
            }
        };
        let (names, ids) = intern(
            entries
                .iter()
                .flat_map(|e| [player(e, e.white()), player(e, e.black())]),
        );
        let white: Vec<u32> = ids.iter().step_by(2).copied().collect();
        let black: Vec<u32> = ids.iter().skip(1).step_by(2).copied().collect();
        let mut players = Names::new(names);
        for &id in &ids {
            players.count(id);
        }
        let (event_names, event) = intern(entries.iter().map(event_name));
        let mut events = Names::new(event_names);
        for &id in &event {
            events.count(id);
        }
        let elo = |v: Option<u32>| v.and_then(|v| u16::try_from(v).ok()).unwrap_or(0);
        let date: Vec<u32> = entries.iter().map(game_date).collect();
        let when: Vec<u64> = entries
            .iter()
            .zip(&date)
            .map(
                |(e, &d)| match (e.tag("UTCDate"), e.tag("UTCTime").and_then(seconds)) {
                    (Some(u), Some(t)) if date_number(u) > 0 => {
                        u64::from(date_number(u)) * 100_000 + u64::from(t)
                    }
                    _ => u64::from(d) * 100_000,
                },
            )
            .collect();
        let text: Vec<bool> = entries.iter().map(is_text).collect();
        let texts = text.iter().filter(|&&t| t).count();
        let decided = entries
            .iter()
            .filter(|e| outcome(e.result()) != Outcome::Unknown)
            .count();
        let eco: Vec<Option<u16>> = entries.iter().map(|e| e.eco().and_then(eco_of)).collect();
        let annotator: Vec<String> = entries
            .iter()
            .map(|e| fold(e.tag("Annotator").unwrap_or("")))
            .collect();
        let white_elo: Vec<u16> = entries.iter().map(|e| elo(e.white_elo())).collect();
        let black_elo: Vec<u16> = entries.iter().map(|e| elo(e.black_elo())).collect();
        let filled = Filled {
            eco: eco.iter().any(Option::is_some),
            annotator: annotator.iter().any(|a| !a.trim().is_empty()),
            elo: white_elo.iter().chain(&black_elo).any(|&e| e > 0),
            year: date.iter().any(|&d| d / 10000 > 0),
        };
        Catalog {
            players,
            events,
            white,
            black,
            event,
            annotator,
            date,
            when,
            books: decided * 2 < entries.len() - texts,
            text,
            texts,
            white_elo,
            black_elo,
            result: entries.iter().map(|e| outcome(e.result())).collect(),
            eco,
            moves: entries
                .iter()
                .map(|e| u16::try_from(e.moves()).unwrap_or(u16::MAX))
                .collect(),
            comments: entries.iter().map(|e| e.flags.has_text_comments).collect(),
            variations: entries.iter().map(|e| e.flags.has_variations).collect(),
            filled,
        }
    }

    /// Which fields at least one game has.
    pub fn filled(&self) -> Filled {
        self.filled
    }

    /// Guiding texts among the records.
    pub fn texts(&self) -> usize {
        self.texts
    }

    /// Most games have no result (a book of chapters).
    pub fn books(&self) -> bool {
        self.books
    }

    pub fn suggest_players(&self, query: &str, limit: usize) -> (Vec<Suggestion>, usize) {
        self.players.suggest(query, limit)
    }

    pub fn suggest_events(&self, query: &str, limit: usize) -> (Vec<Suggestion>, usize) {
        self.events.suggest(query, limit)
    }

    pub fn player_name(&self, id: u32) -> &str {
        self.players.get(id)
    }

    pub fn event_name(&self, id: u32) -> &str {
        self.events.get(id)
    }

    /// The table row of game `index`.
    pub fn row(entry: &GameEntry, moves: u32) -> Row {
        if is_text(entry) {
            return Row {
                kind: Some(RecordKind::Text),
                title: known(entry.white()),
                event: known(entry.event()),
                round: known(entry.tag("Round")),
                annotator: entry.tag("Annotator").unwrap_or("").trim().to_owned(),
                result: "*",
                ..Row::default()
            };
        }
        Row {
            kind: Some(RecordKind::Game),
            white: known(entry.white()),
            black: known(entry.black()),
            white_elo: entry
                .white_elo()
                .and_then(|v| u16::try_from(v).ok())
                .unwrap_or(0),
            black_elo: entry
                .black_elo()
                .and_then(|v| u16::try_from(v).ok())
                .unwrap_or(0),
            result: match outcome(entry.result()) {
                Outcome::WhiteWins => "1-0",
                Outcome::BlackWins => "0-1",
                Outcome::Draw => "1/2-1/2",
                Outcome::Unknown => "*",
            },
            eco: entry
                .eco()
                .and_then(eco_of)
                .map(crate::chessbase::base::eco_text),
            chess960: entry.tag("Variant").is_some_and(|v| v.contains("960")),
            event: known(entry.event()),
            site: known(entry.tag("Site")),
            date: pgn_date(if date_number(entry.date().unwrap_or("")) / 10000 > 0 {
                entry.date().unwrap_or("")
            } else {
                entry.tag("UTCDate").unwrap_or("")
            }),
            round: known(entry.tag("Round")),
            annotator: entry.tag("Annotator").unwrap_or("").trim().to_owned(),
            moves: u8::try_from(moves).unwrap_or(u8::MAX),
            has_variations: entry.flags.has_variations,
            has_comments: entry.flags.has_text_comments,
            title: String::new(),
        }
    }

    /// Indices of the games that pass `filter`, sorted (see `Base::query`).
    pub fn query(
        &self,
        entries: &[GameEntry],
        filter: &Filter,
        sort: SortKey,
        ascending: bool,
        scanned: &AtomicUsize,
        cancel: &AtomicBool,
    ) -> Option<Vec<u32>> {
        let player_ids = name_table(&self.players, &filter.player);
        let opponent_ids = name_table(&self.players, &filter.opponent);
        let event_ids = name_table(&self.events, &filter.event);
        let annotator = words(&filter.annotator);
        // The quick search, word by word: the names it is in; the place, ECO and date of
        // a game are looked at only when no name has the word.
        let quick: Vec<(String, Vec<bool>, Vec<bool>)> = words(&filter.text)
            .into_iter()
            .map(|w| {
                let (p, e) = (self.players.matching(&w), self.events.matching(&w));
                (w, p, e)
            })
            .collect();
        let has = |ids: &Option<Vec<bool>>, id: u32| {
            ids.as_ref()
                .is_none_or(|t| t.get(id as usize).copied().unwrap_or(false))
        };
        let names = player_ids.is_some() || opponent_ids.is_some();
        let relative = filter.player_relative && player_ids.is_some();
        // A guiding text has only a tournament and an annotator: any other condition
        // leaves it out, as in a ChessBase database.
        let game_only = names
            || filter.year_from.is_some()
            || filter.year_to.is_some()
            || filter.elo_min.is_some()
            || !filter.eco.is_empty()
            || !filter.outcomes.is_empty()
            || filter.moves_from.is_some()
            || filter.moves_to.is_some()
            || filter.with_comments
            || filter.with_variations;
        let mut out = Vec::new();
        for i in 0..self.white.len() {
            if i % 65536 == 0 {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                scanned.store(i, Ordering::Relaxed);
            }
            if self.text[i] {
                let at = |t: &Vec<bool>, id: u32| t.get(id as usize).copied().unwrap_or(false);
                if !filter.games_only
                    && !game_only
                    && has(&event_ids, self.event[i])
                    && annotator
                        .iter()
                        .all(|w| self.annotator[i].contains(w.as_str()))
                    && quick.iter().all(|(w, _, events)| {
                        at(events, self.event[i]) || self.annotator[i].contains(w.as_str())
                    })
                {
                    out.push(i as u32);
                }
                continue;
            }
            let (white, black) = (self.white[i], self.black[i]);
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
            if !has(&event_ids, self.event[i])
                || !annotator
                    .iter()
                    .all(|w| self.annotator[i].contains(w.as_str()))
            {
                continue;
            }
            if filter.year_from.is_some() || filter.year_to.is_some() {
                let year = self.date[i] / 10000;
                if year == 0
                    || filter.year_from.is_some_and(|y| year < u32::from(y))
                    || filter.year_to.is_some_and(|y| year > u32::from(y))
                {
                    continue;
                }
            }
            if let Some(min) = filter.elo_min {
                let (we, bl) = (self.white_elo[i], self.black_elo[i]);
                let ok = match filter.elo_rule {
                    EloRule::Both => we >= min && bl >= min,
                    EloRule::Either => we >= min || bl >= min,
                    EloRule::Average => {
                        we > 0 && bl > 0 && (u32::from(we) + u32::from(bl)) / 2 >= u32::from(min)
                    }
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
                let Some(code) = self.eco[i] else { continue };
                if !filter.eco.iter().any(|&(a, b)| (a..=b).contains(&code)) {
                    continue;
                }
            }
            if !filter.outcomes.is_empty() {
                let board = self.result[i];
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
            let moves = self.moves[i];
            if filter.moves_from.is_some_and(|m| moves < m)
                || filter.moves_to.is_some_and(|m| moves > m)
            {
                continue;
            }
            if (filter.with_comments && !self.comments[i])
                || (filter.with_variations && !self.variations[i])
            {
                continue;
            }
            if !quick.is_empty() {
                let at = |t: &Vec<bool>, id: u32| t.get(id as usize).copied().unwrap_or(false);
                let e = entries.get(i);
                let found = quick.iter().all(|(w, players, events)| {
                    at(players, white)
                        || at(players, black)
                        || at(events, self.event[i])
                        || self.annotator[i].contains(w.as_str())
                        || shown_date(self.date[i]).contains(w.as_str())
                        || e.is_some_and(|e| {
                            ["Site", "ECO", "Date", "UTCDate"]
                                .iter()
                                .any(|t| e.tag(t).is_some_and(|v| fold(v).contains(w.as_str())))
                        })
                });
                if !found {
                    continue;
                }
            }
            out.push(i as u32);
        }
        scanned.store(self.white.len(), Ordering::Relaxed);
        self.sort_records(&mut out, sort, ascending);
        Some(out)
    }

    /// Sorts game indices (in file order on entry) by `sort`; games without a value
    /// (no rating, no date) stay at the end in both directions.
    pub fn sort_records(&self, records: &mut Vec<u32>, sort: SortKey, ascending: bool) {
        if sort == SortKey::Number {
            records.sort_unstable();
        } else {
            let mut keyed: Vec<(u64, u32)> = records
                .iter()
                .map(|&i| (self.sort_key(i as usize, sort), i))
                .collect();
            keyed.sort_unstable();
            *records = keyed.into_iter().map(|(_, i)| i).collect();
        }
        if !ascending {
            let split = records
                .iter()
                .position(|&i| self.sort_key(i as usize, sort) == u64::MAX)
                .unwrap_or(records.len());
            records[..split].reverse();
        }
    }

    fn sort_key(&self, i: usize, sort: SortKey) -> u64 {
        if self.text[i] && !matches!(sort, SortKey::Number | SortKey::Event) {
            return u64::MAX;
        }
        let none_if_zero = |v: u32| if v == 0 { u64::MAX } else { u64::from(v) };
        match sort {
            SortKey::Number => i as u64,
            SortKey::White => self.players.rank(self.white[i]),
            SortKey::Black => self.players.rank(self.black[i]),
            SortKey::WhiteElo => none_if_zero(self.white_elo[i].into()),
            SortKey::BlackElo => none_if_zero(self.black_elo[i].into()),
            SortKey::Result => match self.result[i] {
                Outcome::WhiteWins => 0,
                Outcome::Draw => 1,
                Outcome::BlackWins => 2,
                Outcome::Unknown => u64::MAX,
            },
            SortKey::Moves => none_if_zero(self.moves[i].into()),
            SortKey::Eco => self.eco[i].map_or(u64::MAX, u64::from),
            SortKey::Event => self.events.rank(self.event[i]),
            SortKey::Date => {
                if self.when[i] == 0 {
                    u64::MAX
                } else {
                    self.when[i]
                }
            }
            SortKey::AverageElo => {
                let (w, b) = (u64::from(self.white_elo[i]), u64::from(self.black_elo[i]));
                if w == 0 || b == 0 {
                    u64::MAX
                } else {
                    // The average first, then the date.
                    (((w + b) / 2) << 32) | u64::from(self.date[i])
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chessbase::base::NameFilter;
    use chess_core::pgn::scan_games;

    fn catalog(pgn: &str) -> (Vec<GameEntry>, Catalog) {
        let e = scan_games(
            pgn.as_bytes(),
            chess_core::TextEncoding::Utf8,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        let c = Catalog::build(&e);
        (e, c)
    }

    fn find(e: &[GameEntry], c: &Catalog, f: &Filter) -> Vec<u32> {
        c.query(
            e,
            f,
            SortKey::Number,
            true,
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    fn name(t: &str) -> NameFilter {
        NameFilter {
            text: t.into(),
            exact: None,
        }
    }

    const GAMES: &str = concat!(
        "[Event \"Рапид 42, до 11 лет\"][Date \"2026-10-18\"][White \"Вафин  Радель\"][WhiteElo \"1887\"]",
        "[Black \"Макаров Артём\"][BlackElo \"1542\"][Result \"1-0\"]\n1. e4 { [%clk 0:10:07] } c5 2. Nf3 1-0\n",
        "[Event \"Блиц\"][Site \"Москва\"][Date \"2024.05.??\"][White \"Макаров Артём\"][WhiteElo \"1600\"]",
        "[Black \"Hübner, Robert\"][BlackElo \"2600\"][Result \"0-1\"][ECO \"B90a\"]\n1. e4 { A long fight. } c5 (1... e5) 0-1\n",
        "[Event \"Rated Blitz game\"][Site \"https://lichess.org/abc\"][Date \"????.??.??\"][UTCDate \"2023.01.02\"]",
        "[White \"?\"][Black \"-\"][Result \"½-½\"]\n1. d4 1/2-1/2\n",
    );

    #[test]
    fn names_events_and_sides() {
        let (e, c) = catalog(GAMES);
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    player: name("макаров"),
                    ..Filter::default()
                }
            ),
            vec![0, 1]
        );
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    player: name("макаров"),
                    side: Side::White,
                    ..Filter::default()
                }
            ),
            vec![1]
        );
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    player: name("hubner"),
                    ..Filter::default()
                }
            ),
            vec![1]
        );
        // «Артём» is typed «Артем»; the order of the words does not matter.
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    player: name("артем, макаров"),
                    ..Filter::default()
                }
            ),
            vec![0, 1]
        );
        // Spaces squeezed: «Вафин  Радель» is one name.
        assert_eq!(c.suggest_players("вафин", 5).0[0].name, "Вафин Радель");
        // "?" and "-" are no player.
        assert_eq!(c.suggest_players("?", 5).1, 0);
        // Tournaments as ChessBase names them; a link in Site is no place.
        assert_eq!(c.suggest_events("блиц", 5).0[0].name, "Блиц (Москва) 2024");
        assert_eq!(
            c.suggest_events("rated", 5).0[0].name,
            "Rated Blitz game 2023"
        );
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    event: name("рапид 2026"),
                    ..Filter::default()
                }
            ),
            vec![0]
        );
        let (list, total) = c.suggest_players("макар", 5);
        assert_eq!(total, 1);
        assert_eq!(list[0].games, 2);
    }

    #[test]
    fn years_ratings_results_eco_moves_and_comments() {
        let (e, c) = catalog(GAMES);
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    year_from: Some(2025),
                    ..Filter::default()
                }
            ),
            vec![0]
        );
        // The year from UTCDate when Date has none.
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    year_to: Some(2023),
                    ..Filter::default()
                }
            ),
            vec![2]
        );
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    elo_min: Some(1550),
                    ..Filter::default()
                }
            ),
            vec![1]
        );
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    elo_min: Some(2000),
                    elo_rule: EloRule::Either,
                    ..Filter::default()
                }
            ),
            vec![1]
        );
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    outcomes: vec![Outcome::Draw],
                    ..Filter::default()
                }
            ),
            vec![2]
        );
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    eco: vec![(190, 199)],
                    ..Filter::default()
                }
            ),
            vec![1]
        );
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    moves_from: Some(2),
                    ..Filter::default()
                }
            ),
            vec![0]
        );
        // A clock is no comment.
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    with_comments: true,
                    ..Filter::default()
                }
            ),
            vec![1]
        );
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    with_variations: true,
                    ..Filter::default()
                }
            ),
            vec![1]
        );
        assert_eq!(
            c.filled(),
            Filled {
                eco: true,
                annotator: false,
                elo: true,
                year: true
            }
        );
    }

    #[test]
    fn quick_search_finds_each_word_in_some_field() {
        let (e, c) = catalog(GAMES);
        let text = |t: &str| Filter {
            text: t.into(),
            ..Filter::default()
        };
        assert_eq!(find(&e, &c, &text("макаров 2024")), vec![1]);
        assert_eq!(find(&e, &c, &text("москва b90")), vec![1]);
        assert_eq!(find(&e, &c, &text("lichess")), vec![2]);
        assert_eq!(find(&e, &c, &text("вафин блиц")), Vec::<u32>::new());
    }

    #[test]
    fn sorting_and_rows() {
        let (e, c) = catalog(GAMES);
        let mut v = vec![0, 1, 2];
        c.sort_records(&mut v, SortKey::WhiteElo, false);
        assert_eq!(v, vec![0, 1, 2]);
        c.sort_records(&mut v, SortKey::WhiteElo, true);
        // No rating: last in both directions.
        assert_eq!(v, vec![1, 0, 2]);
        c.sort_records(&mut v, SortKey::Date, true);
        assert_eq!(v, vec![2, 1, 0]);
        let r = Catalog::row(&e[0], e[0].moves());
        assert_eq!(r.date, "2026.10.18");
        assert_eq!(r.result, "1-0");
        assert!(!r.has_comments);
        assert_eq!(Catalog::row(&e[1], 1).eco.as_deref(), Some("B90"));
        let r = Catalog::row(&e[2], 1);
        assert_eq!(
            (
                r.white.as_str(),
                r.black.as_str(),
                r.result,
                r.date.as_str()
            ),
            ("", "", "1/2-1/2", "2023.01.02")
        );
    }

    #[test]
    fn downloaded_games_by_the_time_they_were_played() {
        // As a download writes them: one site's games oldest first, then the other's;
        // two games of one day differ by their time.
        let (_, c) = catalog(concat!(
            "[UTCDate \"2026.01.01\"][UTCTime \"10:00:00\"] 1. e4 *\n",
            "[UTCDate \"2026.03.01\"][UTCTime \"09:00:00\"] 1. d4 *\n",
            "[UTCDate \"2026.03.01\"][UTCTime \"21:30:00\"] 1. c4 *\n",
            "[UTCDate \"2026.02.01\"][UTCTime \"10:00:00\"] 1. Nf3 *\n",
        ));
        let mut v = vec![0, 1, 2, 3];
        c.sort_records(&mut v, SortKey::Date, false);
        assert_eq!(v, vec![2, 1, 3, 0]);
    }

    #[test]
    fn guiding_texts_of_a_converted_chessbase_database() {
        let (e, c) = catalog(concat!(
            "[Event \"Course\"][White \"Strong and weak squares\"][Black \"\"][CbText \"1\"][Result \"*\"]\n{ A chapter. } *\n",
            "[Event \"Course\"][White \"Karpov\"][Black \"Kasparov\"][Date \"1985.??.??\"][Result \"1-0\"]\n1. e4 1-0\n",
        ));
        assert_eq!(c.texts(), 1);
        let r = Catalog::row(&e[0], 0);
        assert_eq!(
            (r.kind, r.title.as_str(), r.white.as_str()),
            (Some(RecordKind::Text), "Strong and weak squares", "")
        );
        // A text's title is no player.
        assert_eq!(c.suggest_players("squares", 5).1, 0);
        // Only the tournament and the annotator apply to a text.
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    event: name("course"),
                    ..Filter::default()
                }
            ),
            vec![0, 1]
        );
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    year_from: Some(1900),
                    ..Filter::default()
                }
            ),
            vec![1]
        );
        assert_eq!(
            find(
                &e,
                &c,
                &Filter {
                    games_only: true,
                    ..Filter::default()
                }
            ),
            vec![1]
        );
    }

    #[test]
    fn empty_names_come_last_both_ways() {
        let (_, c) = catalog("[White \"B\"][Black \"?\"] *\n[White \"A\"][Black \"Z\"] *\n");
        let mut v = vec![0, 1];
        c.sort_records(&mut v, SortKey::Black, false);
        assert_eq!(v, vec![1, 0]);
        c.sort_records(&mut v, SortKey::Black, true);
        assert_eq!(v, vec![1, 0]);
    }

    #[test]
    fn the_quick_search_finds_a_date_as_the_table_shows_it() {
        let (e, c) = catalog(GAMES);
        let text = |t: &str| Filter {
            text: t.into(),
            ..Filter::default()
        };
        assert_eq!(find(&e, &c, &text("18.10.2026")), vec![0]);
        assert_eq!(find(&e, &c, &text("2026-10")), vec![0]);
    }

    #[test]
    fn dates() {
        assert_eq!(date_number("2026-10-18"), 20261018);
        assert_eq!(date_number("1997.05.??"), 19970500);
        assert_eq!(date_number("2010"), 20100000);
        assert_eq!(date_number("????.??.??"), 0);
        assert_eq!(date_number(""), 0);
    }
}
