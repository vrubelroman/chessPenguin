//! A game against the computer on the phone (docs/design/android-play.md §31,
//! android-rodent.md): Maia's small model (5M) chooses her moves at a strength of her
//! own scale, by the same tables and rule as on the desktop (`maia::strength`); Rodent IV
//! plays in a style, as on the desktop (`play::rodent`). No clocks on the phone.

use std::sync::Arc;

use maia::strength::choose;
use shakmaty::{CastlingMode, Chess};

use std::sync::Mutex;

use crate::ApiError;
use crate::session::GameSession;

/// How a game stands at a position: over or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum GameEnding {
    /// The side named won by mate.
    Checkmate {
        white_won: bool,
    },
    Stalemate,
    InsufficientMaterial,
    Threefold,
    FiftyMoves,
}

#[uniffi::export]
impl GameSession {
    /// How the game stands at the move at place `at` (its line from the start counts
    /// for repetitions); none while it goes on.
    pub fn ending(&self, at: u32) -> Option<GameEnding> {
        let line = self.read(|g| {
            let id = crate::order(g).get(at as usize).copied()?;
            Some(
                g.path(id)
                    .iter()
                    .filter_map(|n| g.position(*n).cloned())
                    .collect::<Vec<Chess>>(),
            )
        })?;
        Some(match ::play::ending(&line)? {
            ::play::Ending::Checkmate { winner } => GameEnding::Checkmate {
                white_won: winner == shakmaty::Color::White,
            },
            ::play::Ending::Stalemate => GameEnding::Stalemate,
            ::play::Ending::InsufficientMaterial => GameEnding::InsufficientMaterial,
            ::play::Ending::Threefold => GameEnding::Threefold,
            ::play::Ending::FiftyMoves => GameEnding::FiftyMoves,
            // No clocks and no resignations here: the app knows those itself.
            ::play::Ending::Time { .. }
            | ::play::Ending::Resigned { .. }
            | ::play::Ending::DrawAgreed => return None,
        })
    }
}

/// How long the computer "thinks" before its move `ply` of an untimed game
/// (milliseconds): as a person would, quicker with one legal move.
#[uniffi::export]
pub fn move_pause_ms(ply: u32, only_move: bool) -> u64 {
    ::play::pause(None, ply as usize, only_move).as_millis() as u64
}

/// Who plays against the user.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Rival {
    Maia,
    /// Rodent IV in a style of its list (`iv_tal`…).
    Rodent {
        style_id: String,
    },
}

/// The opponent as the game's record names it: «Maia 1500», «Rodent IV 1800 (Tal)».
#[uniffi::export]
pub fn rival_name(rival: Rival, strength: u32) -> String {
    match rival {
        Rival::Maia => format!("Maia {strength}"),
        Rival::Rodent { style_id } => {
            ::play::rodent::name(strength, ::play::rodent::iv_style(&style_id).id)
        }
    }
}

/// The opponent as the app shows it, without the style: «Maia 1500», «Rodent IV 1800».
#[uniffi::export]
pub fn rival_label(rival: Rival, strength: u32) -> String {
    match rival {
        Rival::Maia => format!("Maia {strength}"),
        Rival::Rodent { style_id } => format!(
            "{} {strength}",
            ::play::rodent::iv_style(&style_id).rodent.name()
        ),
    }
}

/// The tags of a game against the computer on the phone (no clocks): `event` in the
/// app's language; the user plays White when `user_white`.
#[uniffi::export]
pub fn computer_game_tags(
    event: String,
    user_white: bool,
    user_name: String,
    rival: Rival,
    strength: u32,
) -> Vec<crate::Tag> {
    let user = if user_white {
        shakmaty::Color::White
    } else {
        shakmaty::Color::Black
    };
    ::play::game_tags(
        &event,
        user,
        &user_name,
        None,
        &rival_name(rival, strength),
        "-",
    )
    .into_iter()
    .map(|(name, value)| crate::Tag { name, value })
    .collect()
}

/// Maia's turns in a row with her position hopeless before she offers to resign.
#[uniffi::export]
pub fn hopeless_turns() -> u32 {
    ::play::HOPELESS_TURNS
}

/// Stockfish judging the positions where Maia is to move (unseen by the user), for her
/// resignation; one for the whole game.
#[derive(uniffi::Object)]
pub struct ResignJudge {
    judge: Mutex<::play::Judge>,
}

#[uniffi::export]
impl ResignJudge {
    /// The engine at `path` with its network `eval_file`.
    #[uniffi::constructor]
    pub fn start(path: String, eval_file: String) -> Arc<ResignJudge> {
        let config = uci::EngineConfig {
            nice: 10,
            ..uci::EngineConfig::new(path)
        };
        let options = if eval_file.is_empty() {
            Vec::new()
        } else {
            vec![("EvalFile".to_owned(), eval_file)]
        };
        Arc::new(ResignJudge {
            judge: Mutex::new(::play::Judge::start(config, options, Box::new(|| {}))),
        })
    }

    /// Judges the position at place `at` of the game (Maia to move there).
    pub fn ask(&self, game: Arc<GameSession>, at: u32) {
        let pos = game.read(|g| {
            let id = crate::order(g).get(at as usize).copied()?;
            g.engine_history(id)
        });
        if let Some(h) = pos {
            let p = match h.start_fen {
                None => uci::EnginePosition::startpos(h.moves),
                Some(fen) => uci::EnginePosition::fen(fen, h.moves),
            };
            self.judge.lock().unwrap_or_else(|e| e.into_inner()).ask(p);
        }
    }

    /// Whether the position asked last is hopeless for Maia, once the answer came.
    pub fn hopeless(&self) -> Option<bool> {
        self.judge
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .answer()
            .map(::play::hopeless)
    }

    /// Drops the request in flight (its position is gone).
    pub fn forget(&self) {
        self.judge
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .forget();
    }

    /// Moves were taken back: the scores of Maia's turns gone are forgotten.
    pub fn forget_scores(&self) {
        self.judge
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .forget_scores();
    }

    /// A draw may be offered now (not right after a refusal).
    pub fn may_offer_draw(&self) -> bool {
        self.judge
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .may_offer_draw()
    }

    /// The user offers a draw: whether Maia accepts (the shared rule, `play::accepts_draw`).
    pub fn offer_draw(&self) -> bool {
        self.judge
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .offer_draw()
    }
}

/// The strengths Maia plays on her own scale, as on the desktop: (weakest, strongest).
#[uniffi::export]
pub fn maia_range() -> Vec<u32> {
    vec![::play::MAIA_MIN, ::play::MAIA_MAX]
}

/// Maia, loaded once for a game.
#[derive(uniffi::Object)]
pub struct MaiaPlayer {
    maia: maia::Maia,
}

#[uniffi::export]
impl MaiaPlayer {
    /// Reads the weights (`maia3-5m.pt`).
    #[uniffi::constructor]
    pub fn load(path: String) -> Result<Arc<MaiaPlayer>, ApiError> {
        let maia =
            maia::Maia::load(std::path::Path::new(&path)).map_err(|e| ApiError::Corrupt {
                detail: e.to_string(),
            })?;
        Ok(Arc::new(MaiaPlayer { maia }))
    }

    /// Maia's move (UCI) in the position at place `at` of the game, playing at
    /// `strength` against a player rated `opponent` (her own strength when unknown).
    /// `random` is in [0, 1). None when the game is over there.
    pub fn choose(
        &self,
        game: Arc<GameSession>,
        at: u32,
        strength: u32,
        opponent: Option<u32>,
        random: f32,
    ) -> Option<String> {
        let history: Vec<Chess> = game.read(|g| {
            let id = crate::order(g).get(at as usize).copied()?;
            Some(
                g.path(id)
                    .iter()
                    .filter_map(|n| g.position(*n).cloned())
                    .collect(),
            )
        })?;
        let strength = strength.clamp(::play::MAIA_MIN, ::play::MAIA_MAX);
        let p = self
            .maia
            .predict(&history, strength, opponent.unwrap_or(strength));
        choose(&p, 1.0, random).map(|m| m.to_uci(CastlingMode::Standard).to_string())
    }
}

// ----- Rodent IV ---------------------------------------------------------------------

/// A character of Rodent IV: its id, its English name (for PGN), its title and what it
/// does (English: the app shows its own translations), a great player or not.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RodentStyle {
    pub id: String,
    pub name: String,
    pub title: String,
    pub about: String,
    pub player: bool,
}

/// Rodent IV's characters (the desktop's list; Rodent V is not on the phone).
#[uniffi::export]
pub fn rodent_styles() -> Vec<RodentStyle> {
    use ::play::rodent::{Gen, STYLES};
    STYLES
        .iter()
        .filter(|s| s.rodent == Gen::IV)
        .map(|s| RodentStyle {
            id: s.id.to_owned(),
            name: s.name.to_owned(),
            title: s.title().to_owned(),
            about: s.about.to_owned(),
            player: s.player,
        })
        .collect()
}

/// The strengths Rodent IV plays (its UCI_Elo): (weakest, strongest).
#[uniffi::export]
pub fn rodent_range() -> Vec<u32> {
    let (lo, hi) = ::play::rodent::Gen::IV.range();
    vec![lo, hi]
}

/// The version of Rodent IV's files (the authors' commit): the personalities copied out
/// of the APK are copied again when it changes.
#[uniffi::export]
pub fn rodent_version() -> String {
    ::play::rodent::COMMIT.to_owned()
}

/// A file of the book pack: where it comes from, where it goes (under the engine's
/// home), its size and SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RodentBook {
    pub url: String,
    pub local: String,
    pub size: u64,
    pub sha256: String,
}

/// The opening books Rodent IV's characters open, downloaded as one pack
/// (android-rodent.md §5).
#[uniffi::export]
pub fn rodent_book_pack() -> Vec<RodentBook> {
    ::play::rodent::book_pack()
        .into_iter()
        .map(|b| RodentBook {
            url: ::play::rodent::file_url(b.file),
            local: b.local,
            size: b.size,
            sha256: b.sha256.to_owned(),
        })
        .collect()
}

/// Rodent IV playing one game (a UCI child process, the shared crate's player).
#[derive(uniffi::Object)]
pub struct RodentPlayer {
    player: ::play::rodent::RodentPlayer,
}

#[uniffi::export]
impl RodentPlayer {
    /// The engine at `exe` with its personalities and books in `home`, playing
    /// `style_id` at `strength` (its UCI_Elo), with its opening book when `use_book`.
    /// It starts in the background: a failure is told by [`RodentPlayer::failure`].
    #[uniffi::constructor]
    pub fn start(
        exe: String,
        home: String,
        style_id: String,
        strength: u32,
        use_book: bool,
    ) -> Arc<RodentPlayer> {
        // Below the app (nice 10), as the analysis: the interface comes first. The
        // engine finds its files by RODENT4HOME (packaging/rodent-iv/android.patch).
        let config = uci::EngineConfig {
            working_dir: Some(home.clone().into()),
            nice: 10,
            env: vec![("RODENT4HOME".into(), home.into())],
            ..uci::EngineConfig::new(exe)
        };
        Arc::new(RodentPlayer {
            player: ::play::rodent::RodentPlayer::start(
                config,
                ::play::rodent::iv_style(&style_id).id,
                Some(strength),
                use_book,
                Box::new(|| {}),
            ),
        })
    }

    /// Rodent's move (UCI) in the position at place `at` of the game, given after
    /// `pause_ms` (it thinks meanwhile). Blocks: called off the main thread. None when
    /// the game is over there, a newer request or `forget` passed it over, or Rodent
    /// failed.
    pub fn choose(&self, game: Arc<GameSession>, at: u32, pause_ms: u64) -> Option<String> {
        let (history, now) = game.read(|g| {
            let id = crate::order(g).get(at as usize).copied()?;
            Some((g.engine_history(id)?, g.position(id)?.clone()))
        })?;
        let pos = match history.start_fen {
            None => uci::EnginePosition::startpos(history.moves),
            Some(fen) => uci::EnginePosition::fen(fen, history.moves),
        };
        let generation = self
            .player
            .ask(pos, now, std::time::Duration::from_millis(pause_ms));
        let (m, _) = self.player.wait(generation)?;
        m.map(|m| m.to_uci(CastlingMode::Standard).to_string())
    }

    /// Drops the request in flight (its position is gone): its `choose` returns none.
    pub fn forget(&self) {
        self.player.forget();
    }

    /// Why Rodent does not play, if it does not.
    pub fn failure(&self) -> Option<RodentFailure> {
        use ::play::rodent::Failure;
        self.player.failure().map(|f| match f {
            Failure::Start(detail) => RodentFailure::DidNotStart { detail },
            // A search not taken: the process is gone in the middle of the game.
            Failure::Stopped | Failure::Search(_) => RodentFailure::Stopped,
        })
    }
}

/// Why Rodent does not play: it did not start (the engine's words), or it stopped
/// during the game (android-rodent.md §6.6).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RodentFailure {
    DidNotStart { detail: String },
    Stopped,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs the weights: `MAIA_WEIGHTS=/path/maia3-5m.pt cargo test -p mobile-api -- --ignored`.
    #[test]
    #[ignore = "needs the weights file (MAIA_WEIGHTS)"]
    fn maia_answers_with_a_legal_move() {
        let path = std::env::var("MAIA_WEIGHTS").expect("MAIA_WEIGHTS");
        let m = MaiaPlayer::load(path).unwrap();
        let s = GameSession::from_pgn("1. e4 *".into()).unwrap();
        let e4 = s.data().nodes.iter().find(|n| n.san == "e4").unwrap().id;
        let uci = m.choose(s.clone(), e4, 1500, None, 0.3).unwrap();
        assert!(s.legal_moves(e4).contains(&uci), "{uci}");
        let mated = GameSession::from_pgn("1. f3 e5 2. g4 Qh4# *".into()).unwrap();
        let end_id = mated
            .data()
            .nodes
            .iter()
            .find(|n| n.san == "Qh4#")
            .unwrap()
            .id;
        assert_eq!(
            mated.ending(end_id),
            Some(GameEnding::Checkmate { white_won: false })
        );
        assert_eq!(mated.ending(0), None);
        let end = mated
            .data()
            .nodes
            .iter()
            .find(|n| n.san == "Qh4#")
            .unwrap()
            .id;
        assert!(m.choose(mated, end, 1500, None, 0.3).is_none());
    }

    #[test]
    fn rodent_is_named_and_listed_as_on_the_desktop() {
        let tags = computer_game_tags(
            "Game".into(),
            true,
            "Roman".into(),
            Rival::Rodent {
                style_id: "iv_tal".into(),
            },
            1800,
        );
        let black = tags.iter().find(|t| t.name == "Black").unwrap();
        assert_eq!(black.value, "Rodent IV 1800 (Tal)");
        assert_eq!(rival_name(Rival::Maia, 1500), "Maia 1500");
        let tal = Rival::Rodent {
            style_id: "iv_tal".into(),
        };
        assert_eq!(rival_label(tal, 1800), "Rodent IV 1800");
        let styles = rodent_styles();
        assert_eq!(styles.len(), 35);
        assert_eq!(styles.iter().filter(|s| s.player).count(), 22);
        assert_eq!(rodent_range(), vec![800, 2800]);
        let pack = rodent_book_pack();
        assert!(
            pack.iter()
                .all(|b| b.url.starts_with("https://raw.githubusercontent.com/")
                    && b.local.starts_with("books/")
                    && b.sha256.len() == 64)
        );
    }

    /// Needs Rodent IV beside its files: `RODENT4=/path/rodent_iv cargo test -p mobile-api -- --ignored`.
    #[test]
    #[ignore = "needs Rodent IV (RODENT4)"]
    fn rodent_answers_with_a_legal_move() {
        let exe = std::env::var("RODENT4").expect("RODENT4");
        let home = std::path::Path::new(&exe)
            .parent()
            .unwrap()
            .display()
            .to_string();
        let r = RodentPlayer::start(exe, home, "iv_karpov".into(), 1600, false);
        let s = GameSession::from_pgn("1. e4 *".into()).unwrap();
        let e4 = s.data().nodes.iter().find(|n| n.san == "e4").unwrap().id;
        let uci = r.choose(s.clone(), e4, 300).expect("a move");
        assert!(s.legal_moves(e4).contains(&uci), "{uci}");
        assert_eq!(r.failure(), None);
    }
}
