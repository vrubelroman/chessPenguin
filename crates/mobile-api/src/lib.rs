//! The ChessPenguin core for the Android app (docs/research/android-stack.md): open a
//! database the way Android hands files over (a file descriptor, or the files of a
//! ChessBase folder), list its games a page at a time, give a game as a tree of moves
//! with the positions, ready for the board and the notation. Bindings: UniFFI.
//!
//! Databases are read only, except the user's own ones in the app's folder
//! (`session.rs`): a game is changed in a [`GameSession`] and saved there.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use chess_core::{Game, MoveKind, NodeId};
use shakmaty::Position;

mod engine;
mod online;
mod review;
mod session;
mod update;

pub use engine::{Analysis, Analyzer, EngineFailure, EngineLine, LineStep};
pub use online::{
    AccountLookup, DownloadState, OnlineAccount, OnlineDatabase, OnlineDownload, OnlineFailure,
    OnlineProblem, OnlineTarget, Site,
};
pub use review::{GameReview, MoveJudgement, ReviewMove, ReviewPoint, ReviewState, SideReview};
pub use session::{Edited, GameSession, own_create, own_remove, own_save};
pub use update::{ApkFile, AppRelease, ReleaseCheck};

uniffi::setup_scaffolding!();

/// What went wrong opening or reading a database, in words for the user (the Kotlin
/// side shows them as they are, or picks its own text by kind).
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ApiError {
    /// The file could not be read.
    #[error("{detail}")]
    Io { detail: String },
    /// A known format that cannot be read (password, ChessBase 2CBH).
    #[error("{detail}")]
    Unsupported { detail: String },
    /// The file is damaged or not a database.
    #[error("{detail}")]
    Corrupt { detail: String },
    /// An archive without a database inside.
    #[error("no PGN or ChessBase database in the archive")]
    Empty,
    /// No game with this number.
    #[error("no game {index}")]
    NoGame { index: u32 },
    /// No move at this place of the game's list.
    #[error("no move {place}")]
    NoNode { place: u32 },
    /// A change of the game that cannot be made (an illegal move, …).
    #[error("{detail}")]
    Edit { detail: String },
}

impl From<chess_import::ImportError> for ApiError {
    fn from(e: chess_import::ImportError) -> Self {
        use chess_import::ImportError as E;
        match e {
            E::Io { source, .. } => ApiError::Io {
                detail: source.to_string(),
            },
            E::Unsupported(u) => ApiError::Unsupported {
                detail: u.to_string(),
            },
            E::Corrupt(m) => ApiError::Corrupt { detail: m },
            E::Empty => ApiError::Empty,
            E::Cancelled => ApiError::Io {
                detail: "cancelled".into(),
            },
        }
    }
}

/// An open database.
#[derive(uniffi::Object)]
pub struct Database {
    db: chess_import::Database,
    format: String,
    notes: Vec<String>,
}

/// One line of the game list.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct GameRow {
    /// The game's number in the database (0-based), for [`Database::game`].
    pub index: u32,
    pub white: String,
    pub black: String,
    /// "1-0", "0-1", "1/2-1/2" or "*".
    pub result: String,
    /// As in PGN: "1858.??.??".
    pub date: String,
    pub event: String,
    pub eco: String,
    pub white_elo: String,
    pub black_elo: String,
}

/// A PGN tag.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Tag {
    pub name: String,
    pub value: String,
}

/// A position in the game: the root (the start position) or the one after a move.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MoveNode {
    /// The node's id (the root is 0); `children` and `parent` refer to these.
    pub id: u32,
    pub parent: Option<u32>,
    /// The moves from here: the main line first, then the variations.
    pub children: Vec<u32>,
    /// SAN of the move that led here ("" for the root), always Latin letters.
    pub san: String,
    /// The move's number and side: 12 and true for "12.Nf3", 12 and false for "12…Nf6".
    pub move_number: u32,
    pub white: bool,
    /// Numeric annotation glyphs ($1 = "!") and their symbols ("!", "+-", …).
    pub nags: Vec<u8>,
    pub nag_symbols: String,
    /// Comments as shown: board commands left out, lines reflowed.
    pub comment_before: String,
    pub comment_after: String,
    /// The position here, and the squares of the move that led to it (for highlighting).
    pub fen: String,
    pub from_square: Option<String>,
    pub to_square: Option<String>,
    /// A null move ("--") of a study.
    pub null_move: bool,
    /// The move in UCI notation, for the engine's history; empty for the root and a
    /// null move (after which the engine gets the position as a FEN).
    pub uci: String,
    /// Arrows (`[%cal]`) and marked squares (`[%csl]`) of the comments after the move.
    pub marks: Vec<Mark>,
    /// "checkmate" or "stalemate" when the game cannot go on from here; "" otherwise.
    pub end: String,
    /// On the main line of the game (not inside a variation).
    pub main_line: bool,
}

/// An arrow (`to` set) or a marked square of a comment, in a comment color:
/// 'R', 'G', 'B', 'Y' or another letter.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Mark {
    pub color: String,
    pub from: String,
    pub to: Option<String>,
}

/// A game ready to show.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct GameData {
    pub tags: Vec<Tag>,
    pub result: String,
    /// Every node: `nodes[0]` is the root; a node's id is its place in this list.
    pub nodes: Vec<MoveNode>,
    /// Problems found reading the game, one line each.
    pub notes: Vec<String>,
}

fn never() -> AtomicBool {
    AtomicBool::new(false)
}

fn wrap(opened: chess_import::Opened) -> Arc<Database> {
    Arc::new(Database {
        db: opened.db,
        format: opened.format.label().to_owned(),
        notes: opened.notes,
    })
}

/// Opens a database file (PGN, `.zip`, `.7z`, `.cbv`) from a file descriptor; the
/// descriptor is taken over and closed (on Android: `ParcelFileDescriptor.detachFd()`).
/// `name` is the file's name, for its extension.
#[cfg(unix)]
#[uniffi::export]
pub fn open_file(name: String, fd: i32) -> Result<Arc<Database>, ApiError> {
    let bytes = read_fd(fd)?;
    open_bytes(name, bytes)
}

#[cfg(unix)]
fn read_fd(fd: i32) -> Result<Vec<u8>, ApiError> {
    use std::os::fd::FromRawFd;
    if fd < 0 {
        return Err(ApiError::Io {
            detail: "no file".into(),
        });
    }
    // SAFETY: the caller hands the descriptor over (detached from its owner); the File
    // owns and closes it.
    read_all(unsafe { std::fs::File::from_raw_fd(fd) })
}

fn read_all(mut file: std::fs::File) -> Result<Vec<u8>, ApiError> {
    use std::io::Read;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| ApiError::Io {
        detail: e.to_string(),
    })?;
    Ok(bytes)
}

/// Opens a database file given as its content.
#[uniffi::export]
pub fn open_bytes(name: String, bytes: Vec<u8>) -> Result<Arc<Database>, ApiError> {
    Ok(wrap(chess_import::open_bytes(
        &name,
        bytes,
        &mut |_| {},
        &never(),
    )?))
}

/// A file of a ChessBase database folder: its name and descriptor (taken over).
#[derive(Debug, Clone, uniffi::Record)]
pub struct FolderFile {
    pub name: String,
    pub fd: i32,
}

/// Opens the ChessBase database `cbh_name` from the files of its folder (the `.cbh`
/// and the files beside it with the same name; others are closed unread).
#[cfg(unix)]
#[uniffi::export]
pub fn open_chessbase(cbh_name: String, files: Vec<FolderFile>) -> Result<Arc<Database>, ApiError> {
    let stem = cbh_name
        .rsplit_once('.')
        .map_or(cbh_name.as_str(), |(s, _)| s);
    // Every descriptor is owned (and so closed) from here on, also when one fails.
    let owned: Vec<(String, Option<std::fs::File>)> = files
        .into_iter()
        .map(|f| (f.name, owned_fd(f.fd)))
        .collect();
    let mut contents = Vec::new();
    for (name, file) in owned {
        if name
            .rsplit_once('.')
            .is_some_and(|(s, _)| s.to_lowercase() == stem.to_lowercase())
        {
            let Some(file) = file else {
                return Err(ApiError::Io {
                    detail: format!("no file {name}"),
                });
            };
            contents.push((name, read_all(file)?));
        }
    }
    Ok(wrap(chess_import::open_chessbase_files(
        &cbh_name,
        &contents,
        &mut |_| {},
        &never(),
    )?))
}

#[cfg(unix)]
fn owned_fd(fd: i32) -> Option<std::fs::File> {
    use std::os::fd::FromRawFd;
    // SAFETY: as in `read_fd`: the descriptor is ours to close.
    (fd >= 0).then(|| unsafe { std::fs::File::from_raw_fd(fd) })
}

/// Opens a database by its path (tests, tools; the app uses descriptors).
#[uniffi::export]
pub fn open_path(path: String) -> Result<Arc<Database>, ApiError> {
    Ok(wrap(chess_import::open(
        std::path::Path::new(&path),
        &mut |_| {},
        &never(),
    )?))
}

#[uniffi::export]
impl Database {
    /// The number of games.
    pub fn len(&self) -> u32 {
        self.db.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.db.is_empty()
    }

    /// What the database was read from ("PGN", "ChessBase", "zip", …).
    pub fn format(&self) -> String {
        self.format.clone()
    }

    /// Remarks about the conversion, one line each.
    pub fn notes(&self) -> Vec<String> {
        self.notes.clone()
    }

    /// Up to `count` lines of the game list from `start` on.
    pub fn rows(&self, start: u32, count: u32) -> Vec<GameRow> {
        let Some(pgn) = self.db.as_pgn() else {
            return Vec::new();
        };
        pgn.entries()
            .iter()
            .skip(start as usize)
            .take(count as usize)
            .map(row)
            .collect()
    }

    /// The numbers of the games whose players, event, site or date contain every word
    /// of `text` (case does not matter); all games for an empty text.
    pub fn find(&self, text: String) -> Vec<u32> {
        let Some(pgn) = self.db.as_pgn() else {
            return Vec::new();
        };
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        pgn.entries()
            .iter()
            .filter(|e| {
                words.is_empty() || {
                    let hay = ["White", "Black", "Event", "Site", "Date"]
                        .iter()
                        .filter_map(|t| e.tag(t))
                        .collect::<Vec<_>>()
                        .join(" ")
                        .to_lowercase();
                    words.iter().all(|w| hay.contains(w.as_str()))
                }
            })
            .map(|e| e.index as u32)
            .collect()
    }

    /// These games (from [`Database::find`]) from the most recent to the oldest, by the
    /// time they were played (`UTCDate` and `UTCTime` when there, else `Date`); games of
    /// the same time keep the newer place in the file first. For downloaded databases,
    /// where both sites' games are appended oldest first.
    pub fn newest_first(&self, indices: Vec<u32>) -> Vec<u32> {
        let Some(pgn) = self.db.as_pgn() else {
            return indices;
        };
        let entries = pgn.entries();
        let when = |i: u32| {
            entries
                .get(i as usize)
                .map(|e| e.played_at())
                .unwrap_or_default()
        };
        let mut keyed: Vec<(String, u32)> = indices.into_iter().map(|i| (when(i), i)).collect();
        keyed.sort_by(|a, b| b.cmp(a));
        keyed.into_iter().map(|(_, i)| i).collect()
    }

    /// The rows of these games (from [`Database::find`]), in this order.
    pub fn rows_of(&self, indices: Vec<u32>) -> Vec<GameRow> {
        let Some(pgn) = self.db.as_pgn() else {
            return Vec::new();
        };
        let entries = pgn.entries();
        indices
            .iter()
            .filter_map(|&i| entries.get(i as usize))
            .map(row)
            .collect()
    }

    /// The game `index` as PGN text, as it is in the file (to copy or share).
    pub fn pgn(&self, index: u32) -> Option<String> {
        self.db.raw_text(index as usize)
    }

    /// The game `index`, ready to show.
    pub fn game(&self, index: u32) -> Result<GameData, ApiError> {
        let parsed = self
            .db
            .load_game(index as usize)
            .ok_or(ApiError::NoGame { index })?;
        Ok(game_data(&parsed.game, &parsed.diagnostics))
    }

    /// The game `index`, to change (moves, variations, its data) and save.
    pub fn session(&self, index: u32) -> Result<Arc<GameSession>, ApiError> {
        let parsed = self
            .db
            .load_game(index as usize)
            .ok_or(ApiError::NoGame { index })?;
        let mut game = parsed.game;
        game.mark_saved();
        Ok(GameSession::wrap(game))
    }

    /// The analysis of the game `index` (its report), with earlier results from the
    /// cache folder `cache_dir` ("" for none) or from `[%eval]` comments of the game.
    pub fn review(&self, index: u32, cache_dir: String) -> Result<Arc<GameReview>, ApiError> {
        let parsed = self
            .db
            .load_game(index as usize)
            .ok_or(ApiError::NoGame { index })?;
        Ok(GameReview::new(parsed.game, cache_dir))
    }
}

/// Node ids in tree order (the root first, each line before its variations' tails),
/// mapped to their places in [`GameData::nodes`].
fn order(game: &Game) -> Vec<NodeId> {
    let mut order = vec![NodeId::ROOT];
    let mut i = 0;
    while i < order.len() {
        let id = order[i];
        order.extend(game.children(id).iter().copied());
        i += 1;
    }
    order
}

/// The place in [`GameData::nodes`] of every node of the game.
fn places(game: &Game) -> std::collections::HashMap<NodeId, u32> {
    order(game)
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i as u32))
        .collect()
}

fn row(e: &chess_core::GameEntry) -> GameRow {
    let tag = |name: &str| e.tag(name).unwrap_or("").to_owned();
    GameRow {
        index: e.index as u32,
        white: tag("White"),
        black: tag("Black"),
        result: tag("Result"),
        date: tag("Date"),
        event: tag("Event"),
        eco: tag("ECO"),
        white_elo: tag("WhiteElo"),
        black_elo: tag("BlackElo"),
    }
}

fn marks(comments: &[String]) -> Vec<Mark> {
    use chess_core::{Annotation, CommentPart, ShapeColor};
    let letter = |c: ShapeColor| {
        match c {
            ShapeColor::Red => 'R',
            ShapeColor::Green => 'G',
            ShapeColor::Blue => 'B',
            ShapeColor::Yellow => 'Y',
            ShapeColor::Other(c) => c,
        }
        .to_string()
    };
    let mut out = Vec::new();
    for c in comments {
        for part in chess_core::parse_comment(c) {
            match part {
                CommentPart::Annotation(Annotation::Arrows(list)) => {
                    out.extend(list.into_iter().map(|(c, from, to)| Mark {
                        color: letter(c),
                        from: from.to_string(),
                        to: Some(to.to_string()),
                    }))
                }
                CommentPart::Annotation(Annotation::Squares(list)) => {
                    out.extend(list.into_iter().map(|(c, sq)| Mark {
                        color: letter(c),
                        from: sq.to_string(),
                        to: None,
                    }))
                }
                _ => {}
            }
        }
    }
    out
}

fn game_data(game: &Game, diagnostics: &[chess_core::Diagnostic]) -> GameData {
    let order = order(game);
    let places = places(game);
    let place = |id: NodeId| places.get(&id).copied();
    let main: std::collections::HashSet<NodeId> = game.mainline().into_iter().collect();
    let nodes = order
        .iter()
        .map(|&id| {
            let node = game.node(id);
            let mv = game.game_move(id);
            let (move_number, color) = game.move_number(id).unwrap_or((0, shakmaty::Color::White));
            let uci = mv.and_then(|m| m.uci());
            let squares = uci
                .as_deref()
                .filter(|u| u.len() >= 4)
                .map(|u| (u[0..2].to_owned(), u[2..4].to_owned()));
            let nags: Vec<u8> = node
                .map(|n| n.nags().iter().map(|g| g.0).collect())
                .unwrap_or_default();
            MoveNode {
                id: place(id).unwrap_or(0),
                parent: game.parent(id).and_then(place),
                children: game.children(id).iter().filter_map(|c| place(*c)).collect(),
                san: mv.map(|m| m.san.clone()).unwrap_or_default(),
                move_number,
                white: color == shakmaty::Color::White,
                nag_symbols: node
                    .map(|n| {
                        n.nags()
                            .iter()
                            .filter_map(|g| g.symbol())
                            .collect::<Vec<_>>()
                            .join("")
                    })
                    .unwrap_or_default(),
                nags,
                comment_before: node
                    .map(|n| chess_core::comment_display_text(n.comments_before()))
                    .unwrap_or_default(),
                comment_after: node
                    .map(|n| chess_core::comment_display_text(n.comments_after()))
                    .unwrap_or_default(),
                fen: game.fen(id).unwrap_or_default(),
                from_square: squares.as_ref().map(|s| s.0.clone()),
                to_square: squares.map(|s| s.1),
                null_move: mv.is_some_and(|m| matches!(m.kind, MoveKind::Null)),
                uci: uci.clone().unwrap_or_default(),
                marks: node.map(|n| marks(n.comments_after())).unwrap_or_default(),
                end: game
                    .position(id)
                    .map(|p| {
                        if p.is_checkmate() {
                            "checkmate"
                        } else if p.is_stalemate() {
                            "stalemate"
                        } else {
                            ""
                        }
                    })
                    .unwrap_or("")
                    .to_owned(),
                main_line: id == NodeId::ROOT || main.contains(&id),
            }
        })
        .collect();
    GameData {
        tags: game
            .tags()
            .iter()
            .map(|(n, v)| Tag {
                name: n.clone(),
                value: v.clone(),
            })
            .collect(),
        result: game.result().as_str().to_owned(),
        nodes,
        notes: diagnostics.iter().map(|d| d.message.clone()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classics() -> Arc<Database> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/classics.pgn");
        open_path(path.into()).expect("the sample opens")
    }

    #[test]
    fn rows_come_a_page_at_a_time() {
        let db = classics();
        assert!(db.len() >= 2);
        let first = db.rows(0, 1);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].index, 0);
        assert!(!first[0].white.is_empty());
        assert_eq!(db.rows(1, 100).len() as u32, db.len() - 1);
        assert!(db.rows(db.len(), 10).is_empty());
    }

    #[test]
    fn comment_marks_and_uci_moves_reach_the_app() {
        let pgn = b"[Event \"t\"]\n\n1. e4 {[%cal Ge2e4,Rd7d5] [%csl Yd5] Center} e5 *\n".to_vec();
        let db = open_bytes("t.pgn".into(), pgn).unwrap();
        let g = db.game(0).unwrap();
        let e4 = &g.nodes[1];
        assert_eq!(e4.uci, "e2e4");
        assert_eq!(e4.comment_after, "Center");
        let m = |c: &str, f: &str, t: Option<&str>| Mark {
            color: c.into(),
            from: f.into(),
            to: t.map(Into::into),
        };
        assert_eq!(
            e4.marks,
            vec![
                m("G", "e2", Some("e4")),
                m("R", "d7", Some("d5")),
                m("Y", "d5", None)
            ]
        );
        assert!(g.nodes[0].uci.is_empty());
        assert!(g.nodes.iter().all(|n| n.end.is_empty()));
        let mate = open_bytes("m.pgn".into(), b"1. f3 e5 2. g4 Qh4# 0-1\n".to_vec()).unwrap();
        let g = mate.game(0).unwrap();
        assert_eq!(g.nodes.last().unwrap().end, "checkmate");
        assert!(db.pgn(0).unwrap().contains("[%cal Ge2e4,Rd7d5]"));
    }

    #[test]
    fn games_are_found_by_words() {
        let db = classics();
        assert_eq!(db.find(String::new()).len() as u32, db.len());
        let morphy = db.find("MORPHY opera".into());
        assert!(!morphy.is_empty());
        let rows = db.rows_of(morphy.clone());
        assert!(
            rows.iter()
                .all(|r| r.white.contains("Morphy") || r.black.contains("Morphy"))
        );
        assert!(db.find("no such player anywhere".into()).is_empty());
    }

    #[test]
    fn a_game_is_a_tree_with_positions() {
        let db = classics();
        let g = db.game(0).expect("a game");
        let root = &g.nodes[0];
        assert_eq!(root.id, 0);
        assert_eq!(root.parent, None);
        assert!(root.fen.starts_with("rnbqkbnr/pppppppp"));
        let first = &g.nodes[root.children[0] as usize];
        assert_eq!(first.parent, Some(0));
        assert!(first.white && first.move_number == 1);
        assert!(first.from_square.is_some() && first.to_square.is_some());
        assert!(first.main_line);
        // Every child points back to its parent, ids are places in the list.
        for (i, n) in g.nodes.iter().enumerate() {
            assert_eq!(n.id as usize, i);
            for &c in &n.children {
                assert_eq!(g.nodes[c as usize].parent, Some(n.id));
            }
        }
        assert!(matches!(db.game(db.len()), Err(ApiError::NoGame { .. })));
    }

    #[test]
    fn comments_and_variations_come_along() {
        // The Opera game of the samples has comments and a variation.
        let db = classics();
        let g = db.game(0).expect("a game");
        assert!(
            g.nodes
                .iter()
                .any(|n| !n.comment_after.is_empty() && !n.comment_after.contains("[%"))
        );
        assert!(g.nodes.iter().any(|n| !n.main_line));
    }

    #[test]
    fn downloaded_games_come_newest_first() {
        let pgn = b"[White \"a\"][UTCDate \"2020.01.02\"][UTCTime \"10:00:00\"]\n\n1. e4 *\n\n\
            [White \"b\"][UTCDate \"2020.01.02\"][UTCTime \"09:00:00\"]\n\n1. e4 *\n\n\
            [White \"c\"][Date \"2021.05.01\"]\n\n1. e4 *\n\n\
            [White \"d\"][UTCDate \"2019.12.31\"][UTCTime \"23:59:59\"]\n\n1. e4 *\n"
            .to_vec();
        let db = open_bytes("t.pgn".into(), pgn).unwrap();
        let order = db.newest_first(db.find(String::new()));
        let names: Vec<String> = db.rows_of(order).into_iter().map(|r| r.white).collect();
        assert_eq!(names, ["c", "a", "b", "d"]);
    }

    #[test]
    fn bytes_open_like_files() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/classics.pgn");
        let bytes = std::fs::read(path).expect("read");
        let db = open_bytes("classics.pgn".into(), bytes).expect("opens");
        assert_eq!(db.len(), classics().len());
        assert_eq!(db.format(), classics().format());
        assert!(matches!(
            open_bytes("x.cbh".into(), vec![0; 46]),
            Err(ApiError::Corrupt { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_descriptor_is_read_and_closed() {
        use std::os::fd::IntoRawFd;
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/classics.pgn");
        let fd = std::fs::File::open(path).expect("open").into_raw_fd();
        let db = open_file("classics.pgn".into(), fd).expect("opens");
        assert_eq!(db.len(), classics().len());
        assert!(open_file("x.pgn".into(), -1).is_err());
    }
}
