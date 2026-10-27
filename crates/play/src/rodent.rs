//! Rodent, the opponent with a style (docs/research/rodent.md, docs/design/rodent-opponent.md,
//! android-rodent.md): its styles and characters, its scales of strength, the files of
//! Rodent IV with the books its characters open, and the engine playing a game in a
//! thread of its own. Shared by the desktop and the Android apps; nothing here draws.

use std::collections::BTreeMap;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use shakmaty::{Chess, Move};

pub use crate::rodent4_files::{COMMIT, FILES, PERSONALITY_BOOKS};

// ----- styles and characters -------------------------------------------------------------

/// Rodent's styles: its personality files, the personality's name (for PGN), what the
/// style is called and does (English: the apps translate it). The players' imitations
/// are marked `player`.
pub struct Style {
    pub id: &'static str,
    pub name: &'static str,
    title: &'static str,
    pub about: &'static str,
    pub player: bool,
    /// Which Rodent plays it: V (a personality file of its own) or IV (a personality of
    /// its list, `personality`; docs/decisions.md «Rodent IV»).
    pub rodent: Gen,
    pub personality: &'static str,
}

impl Style {
    /// The style's name to translate («Attacking»).
    pub fn title(&self) -> &'static str {
        self.title
    }
}

/// Rodent V or Rodent IV: each downloaded on its own, each on its own scale of strength.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gen {
    V,
    IV,
}

impl Gen {
    /// «Rodent V», «Rodent IV»: in the game's record and on the bar.
    pub fn name(self) -> &'static str {
        match self {
            Gen::V => "Rodent V",
            Gen::IV => "Rodent IV",
        }
    }

    /// Its UCI_Elo.
    pub fn range(self) -> (u32, u32) {
        match self {
            Gen::V => (RODENT_MIN, RODENT_MAX),
            Gen::IV => (RODENT_MIN, RODENT_IV_MAX),
        }
    }
}

// Each engine on its own scale, as its authors give it (decisions «Сила соперника — по
// шкале самого движка»): not a person's Elo.

/// Rodent's UCI_Elo.
pub const RODENT_MIN: u32 = 800;
pub const RODENT_MAX: u32 = 3000;
/// Rodent IV's UCI_Elo goes to 2800 (and plays at full strength there).
pub const RODENT_IV_MAX: u32 = 2800;

pub const STYLES: &[Style] = &[
    Style {
        id: "rodent",
        name: "Rodent",
        title: "Balanced",
        about: "Even, all-round play with no bias.",
        player: false,
        rodent: Gen::V,
        personality: "",
    },
    Style {
        id: "ampere",
        name: "Ampere",
        title: "Attacking",
        about: "Active pieces and attack, sound pawn structure.",
        player: false,
        rodent: Gen::V,
        personality: "",
    },
    Style {
        id: "chaotic",
        name: "Chaotic",
        title: "Chaotic",
        about: "Aggressive, tricky openings, careless with pawns.",
        player: false,
        rodent: Gen::V,
        personality: "",
    },
    Style {
        id: "nimzoid",
        name: "Nimzoid",
        title: "Restricting",
        about: "Closed positions, cramping the opponent, old openings.",
        player: false,
        rodent: Gen::V,
        personality: "",
    },
    Style {
        id: "hector",
        name: "Hector",
        title: "Classical",
        about: "Hand-written evaluation, no neural network.",
        player: false,
        rodent: Gen::V,
        personality: "",
    },
    Style {
        id: "tal_hybrid",
        name: "Tal",
        title: "Tal",
        about: "Attacking style tuned on Tal's games.",
        player: true,
        rodent: Gen::V,
        personality: "",
    },
    Style {
        id: "tal_nnue",
        name: "Tal NNUE",
        title: "Tal (network)",
        about: "A neural network trained on Tal's games.",
        player: true,
        rodent: Gen::V,
        personality: "",
    },
    // Rodent IV's characters, then the players it imitates (its personality list).
    Style {
        id: "iv_amanda",
        name: "Amanda",
        title: "Amanda",
        about: "Attacks first, then plays for active pieces.",
        player: false,
        rodent: Gen::IV,
        personality: "Amanda",
    },
    Style {
        id: "iv_cloe",
        name: "Cloe",
        title: "Cloe",
        about: "Likes closed positions.",
        player: false,
        rodent: Gen::IV,
        personality: "Cloe",
    },
    Style {
        id: "iv_deborah",
        name: "Deborah",
        title: "Deborah",
        about: "Defends well and likes her bishops.",
        player: false,
        rodent: Gen::IV,
        personality: "Deborah",
    },
    Style {
        id: "iv_defender",
        name: "Defender",
        title: "Defender",
        about: "Solid and careful; content with a draw.",
        player: false,
        rodent: Gen::IV,
        personality: "Defender",
    },
    Style {
        id: "iv_dynamic",
        name: "Dynamic",
        title: "Dynamic",
        about: "Attacks, likes bishops, sacrifices now and then.",
        player: false,
        rodent: Gen::IV,
        personality: "Dynamic",
    },
    Style {
        id: "iv_grumpy",
        name: "Grumpy",
        title: "Grumpy",
        about: "Attack and restraint; likes blocked positions, avoids draws.",
        player: false,
        rodent: Gen::IV,
        personality: "Grumpy",
    },
    Style {
        id: "iv_partisan",
        name: "Partisan",
        title: "Partisan",
        about: "Attacks after quiet, stealthy openings.",
        player: false,
        rodent: Gen::IV,
        personality: "Partisan",
    },
    Style {
        id: "iv_pawnsacker",
        name: "Pawnsacker",
        title: "Pawnsacker",
        about: "Balanced, but gives up pawns more readily.",
        player: false,
        rodent: Gen::IV,
        personality: "Pawnsacker",
    },
    Style {
        id: "iv_pedrita",
        name: "Pedrita",
        title: "Pedrita",
        about: "Pawns, defence and restraint.",
        player: false,
        rodent: Gen::IV,
        personality: "Pedrita",
    },
    Style {
        id: "iv_preston",
        name: "Preston",
        title: "Preston",
        about: "A materialistic, moderate attacker.",
        player: false,
        rodent: Gen::IV,
        personality: "Preston",
    },
    Style {
        id: "iv_simple",
        name: "Simple",
        title: "Simple",
        about: "The most computer-like: plain, sound chess.",
        player: false,
        rodent: Gen::IV,
        personality: "Simple",
    },
    Style {
        id: "iv_spitfire",
        name: "Spitfire",
        title: "Spitfire",
        about: "Sharp and aggressive.",
        player: false,
        rodent: Gen::IV,
        personality: "Spitfire",
    },
    Style {
        id: "iv_strangler",
        name: "Strangler",
        title: "Strangler",
        about: "A boa constrictor: squeezes slowly, very human-like.",
        player: false,
        rodent: Gen::IV,
        personality: "Strangler",
    },
    Style {
        id: "iv_alekhine",
        name: "Alekhine",
        title: "Alekhine",
        about: "Dynamic attacks and deep combinations.",
        player: true,
        rodent: Gen::IV,
        personality: "Alekhine",
    },
    Style {
        id: "iv_anand",
        name: "Anand",
        title: "Anand",
        about: "Quick, universal play with sharp openings.",
        player: true,
        rodent: Gen::IV,
        personality: "Anand",
    },
    Style {
        id: "iv_anderssen",
        name: "Anderssen",
        title: "Anderssen",
        about: "A romantic attacker of the 19th century: sacrifices.",
        player: true,
        rodent: Gen::IV,
        personality: "Anderssen",
    },
    Style {
        id: "iv_bosboom",
        name: "Bosboom",
        title: "Bosboom",
        about: "Offbeat openings and creative blitz play.",
        player: true,
        rodent: Gen::IV,
        personality: "Bosboom.txt",
    },
    Style {
        id: "iv_botvinnik",
        name: "Botvinnik",
        title: "Botvinnik",
        about: "Scientific, strategic play from deep preparation.",
        player: true,
        rodent: Gen::IV,
        personality: "Botvinnik",
    },
    Style {
        id: "iv_fischer",
        name: "Fischer",
        title: "Fischer",
        about: "Clear, classical and precise.",
        player: true,
        rodent: Gen::IV,
        personality: "Fischer",
    },
    Style {
        id: "iv_karpov",
        name: "Karpov",
        title: "Karpov",
        about: "Positional squeeze, prophylaxis and technique.",
        player: true,
        rodent: Gen::IV,
        personality: "Karpov",
    },
    Style {
        id: "iv_kasparov",
        name: "Kasparov",
        title: "Kasparov",
        about: "Initiative and powerful attack.",
        player: true,
        rodent: Gen::IV,
        personality: "Kasparov",
    },
    Style {
        id: "iv_kortchnoi",
        name: "Kortchnoi",
        title: "Kortchnoi",
        about: "A tenacious defender and counter-attacker.",
        player: true,
        rodent: Gen::IV,
        personality: "Kortchnoi",
    },
    Style {
        id: "iv_larsen",
        name: "Larsen",
        title: "Larsen",
        about: "Original and risky, with offbeat openings.",
        player: true,
        rodent: Gen::IV,
        personality: "Larsen",
    },
    Style {
        id: "iv_lasker",
        name: "Lasker",
        title: "Lasker",
        about: "A practical, psychological fighter.",
        player: true,
        rodent: Gen::IV,
        personality: "Lasker",
    },
    Style {
        id: "iv_marshall",
        name: "Marshall",
        title: "Marshall",
        about: "A swashbuckling attacker with gambits.",
        player: true,
        rodent: Gen::IV,
        personality: "Marshall",
    },
    Style {
        id: "iv_morphy",
        name: "Morphy",
        title: "Morphy",
        about: "Quick development and open lines.",
        player: true,
        rodent: Gen::IV,
        personality: "Morphy",
    },
    Style {
        id: "iv_nimzowitsch",
        name: "Nimzowitsch",
        title: "Nimzowitsch",
        about: "Restraint, blockade and overprotection.",
        player: true,
        rodent: Gen::IV,
        personality: "Nimzowitsch",
    },
    Style {
        id: "iv_petrosian",
        name: "Petrosian",
        title: "Petrosian",
        about: "Prophylaxis and safety first.",
        player: true,
        rodent: Gen::IV,
        personality: "Petrosian",
    },
    Style {
        id: "iv_reti",
        name: "Reti",
        title: "Reti",
        about: "Hypermodern: controls the centre from afar.",
        player: true,
        rodent: Gen::IV,
        personality: "Reti",
    },
    Style {
        id: "iv_rubinstein",
        name: "Rubinstein",
        title: "Rubinstein",
        about: "Classical positional play, great in endgames.",
        player: true,
        rodent: Gen::IV,
        personality: "Rubinstein",
    },
    Style {
        id: "iv_spassky",
        name: "Spassky",
        title: "Spassky",
        about: "A universal player, at home in any position.",
        player: true,
        rodent: Gen::IV,
        personality: "Spassky",
    },
    Style {
        id: "iv_steinitz",
        name: "Steinitz",
        title: "Steinitz",
        about: "The founder of positional play, a stubborn defender.",
        player: true,
        rodent: Gen::IV,
        personality: "Steinitz",
    },
    Style {
        id: "iv_tal",
        name: "Tal",
        title: "Tal (classic)",
        about: "The magician from Riga: sacrifices and attack.",
        player: true,
        rodent: Gen::IV,
        personality: "Tal",
    },
    Style {
        id: "iv_tarrasch",
        name: "Tarrasch",
        title: "Tarrasch",
        about: "Classical rules: space, development, active pieces.",
        player: true,
        rodent: Gen::IV,
        personality: "Tarrasch",
    },
    Style {
        id: "iv_topalov",
        name: "Topalov",
        title: "Topalov",
        about: "Aggressive and dynamic, sacrifices the exchange.",
        player: true,
        rodent: Gen::IV,
        personality: "Topalov",
    },
];

/// The style of this id (the default one for an unknown id).
pub fn style(id: &str) -> &'static Style {
    STYLES.iter().find(|s| s.id == id).unwrap_or(&STYLES[0])
}

/// Rodent IV's character of this id (Tal for an unknown one, or one of Rodent V: a
/// phone has Rodent IV only, android-rodent.md §7.1).
pub fn iv_style(id: &str) -> &'static Style {
    STYLES
        .iter()
        .find(|s| s.id == id && s.rodent == Gen::IV)
        .unwrap_or_else(|| style("iv_tal"))
}

/// Rodent as a game names it: «Rodent V 2400» (the default style unnamed), «Rodent V 2400
/// (Tal)», «Rodent IV 1800 (Tal)» — the personality's English name: PGN is not translated.
pub fn name(strength: u32, style_id: &str) -> String {
    let st = style(style_id);
    if st.id == STYLES[0].id {
        format!("{} {strength}", st.rodent.name())
    } else {
        format!("{} {strength} ({})", st.rodent.name(), st.name)
    }
}

// ----- Rodent IV's files ---------------------------------------------------------------

/// Books three personalities name by a path not in the authors' files (a case or a dash
/// off): they are read under that name (the engine takes books only from the
/// personality's file). (file in the repository, name it is read by)
pub const BOOK_NAMES: &[(&str, &str)] = &[
    ("books/guide/Strangler.bin", "books/guide/strangler.bin"),
    (
        "books/players/ph-spassky2.bin",
        "books/players/ph_spassky2.bin",
    ),
    ("books/guide.bin", "books/guide/guide.bin"),
];

/// A path of the repository as it goes into a URL («#readme.txt» has a «#»).
pub fn url_path(path: &str) -> String {
    path.replace('#', "%23").replace(' ', "%20")
}

/// Where a file of the authors' repository is downloaded from (at `COMMIT`).
pub fn file_url(path: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/nescitus/rodent-iv/{COMMIT}/{}",
        url_path(path)
    )
}

/// An opening book of Rodent IV to download: its file in the authors' repository, where
/// the engine reads it (relative to its home folder), its size and SHA-256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Book {
    pub file: &'static str,
    pub local: String,
    pub size: u64,
    pub sha256: &'static str,
}

/// The books Rodent IV's characters open (the guide and the main book of each), each
/// once, under the name it is read by: the phone's book pack (android-rodent.md §5), not
/// the books nobody uses.
pub fn book_pack() -> Vec<Book> {
    let mut books = BTreeMap::new();
    for st in STYLES.iter().filter(|s| s.rodent == Gen::IV) {
        let Some(&(_, guide, main)) = PERSONALITY_BOOKS.iter().find(|p| p.0 == st.personality)
        else {
            continue;
        };
        for book in [guide, main] {
            let local = format!("books/{book}");
            let file = BOOK_NAMES
                .iter()
                .find(|(_, name)| *name == local)
                .map_or(local.as_str(), |(file, _)| file);
            if let Some(&(file, size, sha256)) = FILES.iter().find(|f| f.0 == file) {
                books.entry(local.clone()).or_insert(Book {
                    file,
                    local,
                    size,
                    sha256,
                });
            }
        }
    }
    books.into_values().collect()
}

// ----- Rodent playing a game -----------------------------------------------------------

/// Why Rodent does not play.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// It did not start, or did not take its options (the engine's words).
    Start(String),
    /// Its process ended during the game.
    Stopped,
    /// A search could not be asked for (the engine's words).
    Search(String),
}

/// A move asked for: the game for the engine, its position now, the pause.
struct Request {
    generation: u64,
    pos: uci::EnginePosition,
    now: Chess,
    pause: Duration,
}

/// Rodent's move, or none (no legal move it gave), and the time it took.
pub type Answer = (Option<Move>, Duration);

#[derive(Default)]
struct State {
    /// The request asked last (bumped by `forget`).
    generation: u64,
    /// The newest request answered or passed over, and the answer of the newest.
    answered: u64,
    answer: Option<(u64, Answer)>,
    failure: Option<Failure>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn fail(&self, f: Failure) {
        self.lock().failure = Some(f);
        self.changed.notify_all();
    }
}

/// Rodent playing one game, in its own thread (a UCI child process): it thinks during
/// the pause a move should take (a game's own clock is not its business: Maia's pause
/// rule paces it) and answers after it. Only the newest request counts; the answer to
/// an older one (after a take-back) is dropped.
pub struct RodentPlayer {
    tx: Sender<Request>,
    shared: Arc<Shared>,
}

impl RodentPlayer {
    /// The engine of `config` (its folder, or its home in the environment: Android)
    /// playing `style_id` at `uci_elo` (none: full strength), with its opening book when
    /// `use_book`. `wake` is called when an answer or a failure comes (the app draws again).
    pub fn start(
        config: uci::EngineConfig,
        style_id: &str,
        uci_elo: Option<u32>,
        use_book: bool,
        wake: Box<dyn Fn() + Send>,
    ) -> RodentPlayer {
        let (tx, rx) = channel::<Request>();
        let shared = Arc::new(Shared::default());
        let sh = shared.clone();
        let st = style(style_id);
        let _ = std::thread::Builder::new()
            .name("rodent".into())
            .spawn(move || {
                let fail = |f: Failure| {
                    sh.fail(f);
                    wake();
                };
                let engine = match uci::Engine::spawn_with(config, Box::new(|| {})) {
                    Ok(e) => e,
                    Err(e) => return fail(Failure::Start(e.to_string())),
                };
                // V reads a personality file; IV picks one of its list (with its books).
                let book = use_book.to_string();
                let mut setup = match st.rodent {
                    Gen::V => vec![
                        ("PersonalityFile", format!("personalities/{}.txt", st.id)),
                        ("OwnBook", book),
                    ],
                    Gen::IV => vec![
                        ("Personality", st.personality.to_owned()),
                        ("UseBook", book),
                    ],
                };
                if let Some(elo) = uci_elo {
                    setup.push(("UCI_LimitStrength", "true".to_owned()));
                    setup.push(("UCI_Elo", elo.to_string()));
                }
                let ready = engine.set_threads(1).and_then(|()| engine.set_hash_mb(64));
                if let Err(e) = ready {
                    return fail(Failure::Start(e.to_string()));
                }
                for (name, value) in &setup {
                    if let Err(e) = engine.set_option(name, Some(value)) {
                        return fail(Failure::Start(e.to_string()));
                    }
                }
                let _ = engine.new_game();
                while let Ok(mut r) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        r = newer;
                    }
                    let started = Instant::now();
                    let id = uci::SearchId(r.generation);
                    // Its own limit (from UCI_Elo) ends the search early; at full strength
                    // it thinks for the pause.
                    let think = r.pause.max(Duration::from_millis(300));
                    if let Err(e) = engine.search(r.pos, id, uci::SearchLimits::MoveTime(think)) {
                        return fail(Failure::Search(e.to_string()));
                    }
                    let mut stopped: Option<Instant> = None;
                    let best = loop {
                        match engine.recv_timeout(Duration::from_millis(200)) {
                            Some(uci::EngineEvent::BestMove {
                                search_id, best: b, ..
                            }) if search_id == id => break b,
                            Some(uci::EngineEvent::Terminated(_)) => {
                                return fail(Failure::Stopped);
                            }
                            // A search that does not end: stopped, then given up on.
                            _ => match stopped {
                                None if started.elapsed() > think + Duration::from_secs(20) => {
                                    let _ = engine.stop();
                                    stopped = Some(Instant::now());
                                }
                                Some(t) if t.elapsed() > Duration::from_secs(5) => {
                                    return fail(Failure::Stopped);
                                }
                                _ => {}
                            },
                        }
                    };
                    let m = best
                        .and_then(|b| b.parse::<shakmaty::uci::UciMove>().ok())
                        .and_then(|u| u.to_move(&r.now).ok());
                    std::thread::sleep(r.pause.saturating_sub(started.elapsed()));
                    let spent = r.pause.max(started.elapsed());
                    {
                        let mut s = sh.lock();
                        s.answered = s.answered.max(r.generation);
                        s.answer = Some((r.generation, (m, spent)));
                    }
                    sh.changed.notify_all();
                    wake();
                }
                engine.quit();
            });
        RodentPlayer { tx, shared }
    }

    /// Asks for a move in `now` (`pos`: the game to it for the engine) after `pause`;
    /// the request's number, for [`RodentPlayer::wait`].
    pub fn ask(&self, pos: uci::EnginePosition, now: Chess, pause: Duration) -> u64 {
        let generation = {
            let mut s = self.shared.lock();
            s.generation += 1;
            s.generation
        };
        let _ = self.tx.send(Request {
            generation,
            pos,
            now,
            pause,
        });
        generation
    }

    /// Forgets what was asked (its answer will be dropped; a wait for it ends).
    pub fn forget(&self) {
        self.shared.lock().generation += 1;
        self.shared.changed.notify_all();
    }

    /// The answer to the request asked last, once it has come (given once).
    pub fn answer(&self) -> Option<Answer> {
        let mut s = self.shared.lock();
        let generation = s.generation;
        match s.answer {
            Some((g, _)) if g == generation => s.answer.take().map(|(_, a)| a),
            _ => None,
        }
    }

    /// Waits for the answer to request `generation`: none when it was forgotten or
    /// passed over by a newer one, or Rodent failed.
    pub fn wait(&self, generation: u64) -> Option<Answer> {
        let mut s = self.shared.lock();
        while s.generation == generation && s.answered < generation && s.failure.is_none() {
            s = self
                .shared
                .changed
                .wait(s)
                .unwrap_or_else(|e| e.into_inner());
        }
        match s.answer {
            Some((g, _)) if g == generation && s.generation == generation => {
                s.answer.take().map(|(_, a)| a)
            }
            _ => None,
        }
    }

    /// Why Rodent does not play, if it does not.
    pub fn failure(&self) -> Option<Failure> {
        self.shared.lock().failure.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_is_where_the_engine_looks_for_it() {
        assert_eq!(COMMIT.len(), 40);
        assert!(FILES.iter().any(|f| f.0 == "personalities/basic.ini"));
        assert!(FILES.iter().any(|f| f.0 == "personalities/tal.txt"));
        assert!(FILES.iter().any(|f| f.0 == "books/rodent.bin"));
        assert!(FILES.iter().all(|f| f.2.len() == 64 && !f.0.contains("..")));
        assert_eq!(url_path("books/#readme.txt"), "books/%23readme.txt");
        // The books read under another name are in the authors' files.
        for (file, _) in BOOK_NAMES {
            assert!(FILES.iter().any(|f| f.0 == *file), "{file}");
        }
    }

    #[test]
    fn the_pack_has_the_books_of_every_character_and_no_other() {
        let pack = book_pack();
        let has = |local: &str| pack.iter().any(|b| b.local == local);
        let iv: Vec<&Style> = STYLES.iter().filter(|s| s.rodent == Gen::IV).collect();
        assert_eq!(iv.len(), 35);
        for st in iv {
            let (_, guide, main) = PERSONALITY_BOOKS
                .iter()
                .find(|p| p.0 == st.personality)
                .unwrap_or_else(|| panic!("no books for {}", st.personality));
            assert!(has(&format!("books/{guide}")), "{guide}");
            assert!(has(&format!("books/{main}")), "{main}");
        }
        // The three named otherwise come from their files in the repository.
        let spassky = pack
            .iter()
            .find(|b| b.local == "books/players/ph_spassky2.bin")
            .unwrap();
        assert_eq!(spassky.file, "books/players/ph-spassky2.bin");
        assert!(has("books/guide/strangler.bin") && has("books/guide/guide.bin"));
        // Nobody's books stay out: 59 MB of the 69.
        assert!(!has("books/guide/ph-exoticbook.bin"));
        let total: u64 = pack.iter().map(|b| b.size).sum();
        assert_eq!((total + 500_000) / 1_000_000, 59);
        assert!(
            file_url(spassky.file).ends_with(&format!("/{COMMIT}/books/players/ph-spassky2.bin"))
        );
    }

    #[test]
    fn rodent_is_named_with_its_strength_and_personality() {
        assert_eq!(name(1800, "iv_tal"), "Rodent IV 1800 (Tal)");
        assert_eq!(name(2400, "rodent"), "Rodent V 2400");
        assert_eq!(name(2400, "tal_hybrid"), "Rodent V 2400 (Tal)");
        assert_eq!(style("iv_spassky").rodent.range(), (800, 2800));
        // An unknown style (of a later version) is the default one; of Rodent IV, Tal.
        assert_eq!(style("nobody").id, "rodent");
        assert_eq!(iv_style("nobody").id, "iv_tal");
        assert_eq!(iv_style("ampere").id, "iv_tal");
        assert_eq!(iv_style("iv_karpov").id, "iv_karpov");
    }

    /// Needs Rodent IV and its files: `RODENT4=/path/rodent_iv cargo test -p play -- --ignored`
    /// (the folder of the executable has `personalities/` and `books/`).
    #[test]
    #[ignore = "needs Rodent IV (RODENT4)"]
    fn rodent_answers_the_newest_request_only() {
        let exe = std::env::var("RODENT4").expect("RODENT4");
        let player = RodentPlayer::start(
            uci::EngineConfig::new(exe),
            "iv_tal",
            Some(1800),
            true,
            Box::new(|| {}),
        );
        let start = || uci::EnginePosition::startpos(Vec::<String>::new());
        let pause = Duration::from_millis(400);
        // Asked twice: the first is passed over, the second answered with a legal move.
        let first = player.ask(start(), Chess::default(), pause);
        let second = player.ask(start(), Chess::default(), pause);
        assert_eq!(player.wait(first), None);
        let (m, spent) = player.wait(second).expect("an answer");
        assert!(m.is_some(), "{:?}", player.failure());
        assert!(spent >= pause);
        // A request forgotten ends its wait at once.
        let third = player.ask(start(), Chess::default(), pause);
        player.forget();
        assert_eq!(player.wait(third), None);
        assert_eq!(player.failure(), None);
    }
}
