//! A game being entered or changed (docs/design/android-play.md §29–30): moves played
//! on the board go into its tree (a new move at the end extends the line, an existing
//! one is followed, any other becomes a variation; nothing is overwritten), with undo,
//! the game's data and its PGN for saving. And the user's own databases: PGN files in
//! the app's folder, created, added to and changed here (never a file of someone else).

use std::sync::{Arc, Mutex};

use chess_core::{EditError, Game, GameResult, NodeId};
use shakmaty::uci::UciMove;
use shakmaty::{CastlingMode, Position};

use crate::{ApiError, GameData, Tag, game_data, order, places};

/// A game after a change: all of it again (the ids are places in `data.nodes`, which a
/// change may move) and the node to show.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Edited {
    pub data: GameData,
    pub cursor: u32,
}

/// A game that can be changed.
#[derive(uniffi::Object)]
pub struct GameSession {
    game: Mutex<Game>,
}

impl GameSession {
    pub(crate) fn wrap(game: Game) -> Arc<GameSession> {
        Arc::new(GameSession {
            game: Mutex::new(game),
        })
    }

    fn with<R>(&self, f: impl FnOnce(&mut Game) -> R) -> R {
        let mut g = self.game.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut g)
    }
}

/// The node at place `at` of the game's list.
fn node(game: &Game, at: u32) -> Result<NodeId, ApiError> {
    order(game)
        .get(at as usize)
        .copied()
        .ok_or(ApiError::NoNode { place: at })
}

fn edited(game: &Game, cursor: NodeId) -> Edited {
    Edited {
        data: game_data(game, &[]),
        cursor: places(game).get(&cursor).copied().unwrap_or(0),
    }
}

impl From<EditError> for ApiError {
    fn from(e: EditError) -> Self {
        ApiError::Edit {
            detail: e.to_string(),
        }
    }
}

#[uniffi::export]
impl GameSession {
    /// A new game from the start position, dated today (`date`: "2026.10.26").
    #[uniffi::constructor]
    pub fn new_game(date: String) -> Arc<GameSession> {
        let mut g = Game::new();
        if !date.is_empty() {
            g.set_tag("Date", &date);
        }
        g.mark_saved();
        GameSession::wrap(g)
    }

    /// The first game of a PGN text.
    #[uniffi::constructor]
    pub fn from_pgn(pgn: String) -> Result<Arc<GameSession>, ApiError> {
        let parsed = chess_core::pgn::parse_game(pgn.as_bytes(), chess_core::TextEncoding::Utf8);
        let mut g = parsed.game;
        g.mark_saved();
        Ok(GameSession::wrap(g))
    }

    /// The whole game, ready to show.
    pub fn data(&self) -> GameData {
        self.with(|g| game_data(g, &[]))
    }

    /// The legal moves in the position at place `at`, in UCI ("e2e4", "e7e8q"); none
    /// after a null move's position cannot go on, at checkmate or stalemate.
    pub fn legal_moves(&self, at: u32) -> Vec<String> {
        self.with(|g| {
            let Ok(id) = node(g, at) else {
                return Vec::new();
            };
            g.position(id)
                .map(|p| {
                    p.legal_moves()
                        .iter()
                        .map(|m| m.to_uci(CastlingMode::Standard).to_string())
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// Plays `uci` from the node at place `at`: the existing continuation when there is
    /// one (nothing changes), else a new move — the main line where the line ends, a
    /// variation elsewhere. One undo step.
    pub fn play(&self, at: u32, uci: String) -> Result<Edited, ApiError> {
        self.with(|g| {
            let id = node(g, at)?;
            let pos = g
                .position(id)
                .ok_or(ApiError::NoNode { place: at })?
                .clone();
            let m = uci
                .parse::<UciMove>()
                .ok()
                .and_then(|u| u.to_move(&pos).ok())
                .ok_or_else(|| ApiError::Edit {
                    detail: format!("illegal move {uci}"),
                })?;
            let new = g.add_move(id, m)?;
            Ok(edited(g, new))
        })
    }

    /// Removes the move at place `at` with all that follows it; shows its parent.
    pub fn delete_from(&self, at: u32) -> Result<Edited, ApiError> {
        self.with(|g| {
            let id = node(g, at)?;
            let parent = g.delete_variation(id)?;
            Ok(edited(g, parent))
        })
    }

    /// Makes the line of the move at place `at` the main one at its branch.
    pub fn make_main(&self, at: u32) -> Result<Edited, ApiError> {
        self.with(|g| {
            let id = node(g, at)?;
            g.promote_variation(id)?;
            Ok(edited(g, id))
        })
    }

    /// Adds a line of moves (an engine's) from place `at`, as `play` does each move.
    pub fn add_line(&self, at: u32, uci: Vec<String>) -> Result<Edited, ApiError> {
        self.with(|g| {
            let id = node(g, at)?;
            let last = g.add_uci_line(id, &uci)?;
            Ok(edited(g, last))
        })
    }

    pub fn can_undo(&self) -> bool {
        self.with(|g| g.can_undo())
    }

    pub fn can_redo(&self) -> bool {
        self.with(|g| g.can_redo())
    }

    /// Takes the last change back; `None` when there is none.
    pub fn undo(&self) -> Option<Edited> {
        self.with(|g| g.undo().map(|o| edited(g, o.cursor)))
    }

    pub fn redo(&self) -> Option<Edited> {
        self.with(|g| g.redo().map(|o| edited(g, o.cursor)))
    }

    /// Sets the game's tags (an empty value removes the tag; one of the Seven Tag Roster
    /// becomes "?") and its result ("1-0", "0-1", "1/2-1/2", "*").
    pub fn set_data(&self, tags: Vec<Tag>, result: String) {
        self.with(|g| {
            for t in tags {
                if t.value.trim().is_empty() {
                    g.remove_tag(&t.name);
                } else {
                    g.set_tag(&t.name, t.value.trim());
                }
            }
            if let Some(r) = GameResult::from_pgn(&result) {
                g.set_result(r);
            }
            // The Seven Tag Roster first, in its order, "?" where unknown (as the desktop).
            g.order_roster();
        })
    }

    /// Changed since it was opened or last saved.
    pub fn is_modified(&self) -> bool {
        self.with(|g| g.is_modified())
    }

    pub fn mark_saved(&self) {
        self.with(|g| g.mark_saved())
    }

    /// The game as PGN, for saving or sharing.
    pub fn pgn(&self) -> String {
        self.with(|g| g.to_pgn())
    }
}

// ----- own databases ----------------------------------------------------------------

fn io(e: impl std::fmt::Display) -> ApiError {
    ApiError::Io {
        detail: e.to_string(),
    }
}

/// Opens the own database at `path` for writing (read just now, so it is current).
fn own(path: &str) -> Result<chess_import::Database, ApiError> {
    Ok(chess_import::open(std::path::Path::new(path), &mut |_| {}, &crate::never())?.db)
}

/// Makes a new empty own database at `path` (refused when the file exists).
#[uniffi::export]
pub fn own_create(path: String) -> Result<(), ApiError> {
    let path = std::path::Path::new(&path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|f| f.sync_all())
        .map_err(io)
}

/// Saves the game `pgn` into the own database at `path`: as game `index` (replacing
/// it), or added at the end when `index` is `None`. Returns the game's number.
#[uniffi::export]
pub fn own_save(path: String, index: Option<u32>, pgn: String) -> Result<u32, ApiError> {
    let db = own(&path)?;
    let file = db.as_pgn().ok_or_else(|| ApiError::Unsupported {
        detail: "not a PGN file".into(),
    })?;
    match index {
        Some(i) => {
            chess_core::pgn::store::replace(file, i as usize, &pgn).map_err(io)?;
            Ok(i)
        }
        None => {
            chess_core::pgn::store::append(file, &pgn).map_err(io)?;
            Ok(file.len() as u32)
        }
    }
}

/// Removes game `index` from the own database at `path`.
#[uniffi::export]
pub fn own_remove(path: String, index: u32) -> Result<(), ApiError> {
    let db = own(&path)?;
    let file = db.as_pgn().ok_or_else(|| ApiError::Unsupported {
        detail: "not a PGN file".into(),
    })?;
    chess_core::pgn::store::remove(file, index as usize)
        .map(|_| ())
        .map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn san_line(d: &GameData) -> Vec<String> {
        let mut out = Vec::new();
        let mut at = 0usize;
        while let Some(&c) = d.nodes[at].children.first() {
            out.push(d.nodes[c as usize].san.clone());
            at = c as usize;
        }
        out
    }

    #[test]
    fn moves_extend_the_line_and_branch_into_variations() {
        let s = GameSession::new_game("2026.10.26".into());
        let e = s.play(0, "e2e4".into()).unwrap();
        let e = s.play(e.cursor, "e7e5".into()).unwrap();
        assert_eq!(san_line(&e.data), ["e4", "e5"]);
        // The same move again: followed, not added.
        let again = s.play(0, "e2e4".into()).unwrap();
        assert_eq!(again.data.nodes.len(), 3);
        // Another first move: a variation, the main line kept.
        let v = s.play(0, "d2d4".into()).unwrap();
        assert_eq!(san_line(&v.data), ["e4", "e5"]);
        assert_eq!(v.data.nodes[0].children.len(), 2);
        assert!(!v.data.nodes[v.cursor as usize].main_line);
        // Undo takes the variation back; redo brings it again.
        let u = s.undo().unwrap();
        assert_eq!(u.data.nodes[0].children.len(), 1);
        assert!(s.redo().is_some());
        assert!(s.is_modified());
    }

    #[test]
    fn a_move_keeps_its_id_when_the_game_changes() {
        let s = GameSession::from_pgn("1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *".into()).unwrap();
        let before = s.data();
        let nf3 = before.nodes.iter().find(|n| n.san == "Nf3").unwrap().id;
        // A variation at the first move, then removed: Nf3 stays where it was.
        let v = s.play(0, "d2d4".into()).unwrap();
        assert_eq!(v.data.nodes[nf3 as usize].san, "Nf3");
        let gone = s.delete_from(v.cursor).unwrap();
        assert_eq!(gone.data.nodes[nf3 as usize].san, "Nf3");
        // The removed move is reached from nowhere.
        assert!(gone.data.nodes[v.cursor as usize].parent.is_none());
        assert!(!gone.data.nodes[0].children.contains(&v.cursor));
        // Undo brings it back under the same id.
        let back = s.undo().unwrap();
        assert!(back.data.nodes[0].children.contains(&v.cursor));
    }

    #[test]
    fn an_illegal_move_changes_nothing() {
        let s = GameSession::new_game(String::new());
        assert!(s.play(0, "e2e5".into()).is_err());
        assert!(!s.can_undo());
        assert_eq!(s.legal_moves(0).len(), 20);
    }

    #[test]
    fn variations_become_main_and_go() {
        let s = GameSession::new_game(String::new());
        s.play(0, "e2e4".into()).unwrap();
        let d4 = s.play(0, "d2d4".into()).unwrap();
        let m = s.make_main(d4.cursor).unwrap();
        assert_eq!(san_line(&m.data), ["d4"]);
        let gone = s.delete_from(m.data.nodes[0].children[0]).unwrap();
        assert_eq!(san_line(&gone.data), ["e4"]);
        assert_eq!(gone.cursor, 0);
    }

    #[test]
    fn games_are_saved_into_own_databases() {
        let dir = std::env::temp_dir().join(format!("cp-own-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("own").join("Мои партии.pgn");
        let p = path.to_string_lossy().into_owned();
        own_create(p.clone()).unwrap();
        assert!(
            own_create(p.clone()).is_err(),
            "an existing file is not replaced"
        );

        let s = GameSession::new_game("2026.10.26".into());
        s.play(0, "e2e4".into()).unwrap();
        s.set_data(
            vec![Tag {
                name: "White".into(),
                value: "Костя".into(),
            }],
            "1-0".into(),
        );
        assert_eq!(own_save(p.clone(), None, s.pgn()).unwrap(), 0);
        assert_eq!(own_save(p.clone(), None, s.pgn()).unwrap(), 1);
        let s2 = GameSession::from_pgn(s.pgn()).unwrap();
        s2.play(1, "e7e5".into()).unwrap();
        assert_eq!(own_save(p.clone(), Some(0), s2.pgn()).unwrap(), 0);

        // The roster stays whole and in order, unknown values "?".
        let pgn = s.pgn();
        let order: Vec<&str> = pgn
            .lines()
            .take(7)
            .map(|l| l.split(' ').next().unwrap())
            .collect();
        assert_eq!(
            order,
            [
                "[Event", "[Site", "[Date", "[Round", "[White", "[Black", "[Result"
            ]
        );
        assert!(pgn.contains("[Event \"?\"]"));
        let db = crate::open_path(p.clone()).unwrap();
        assert_eq!(db.len(), 2);
        let first = db.game(0).unwrap();
        assert_eq!(san_line(&first), ["e4", "e5"]);
        assert_eq!(first.result, "1-0");
        assert!(
            first
                .tags
                .iter()
                .any(|t| t.name == "White" && t.value == "Костя")
        );
        own_remove(p.clone(), 1).unwrap();
        assert_eq!(crate::open_path(p).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
