//! The engine of the game analysis and the search for opening errors: ChessPenguin's
//! Stockfish 19, or else a Stockfish 19 of the user's (from the settings, or the one in
//! `PATH`) that searches exactly as ours (decision of 2026-10-31). Whether it does is
//! told by `game_review::probe` on a thread of its own: an engine not called
//! «Stockfish 19» is turned down at its `uci` answer, without a search; one that is
//! makes three short searches of the analysis's fixed mode. The verdict is remembered
//! for the real file (links followed) with its size and time, and with the probe's
//! reference: a file is probed once, again when it or the reference changes. An engine
//! that did not start is not remembered: it is tried again at the next request.

use crate::engines::EngineLocation;

/// Why the user's engine does not do for the analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// Another engine, or another version of Stockfish.
    Other,
    /// It calls itself Stockfish 19 but searches otherwise (another build or network).
    Differs,
    /// It did not start or did not answer.
    NotStarted,
}

/// The user's engine that does not do: its name (`id name`, else what the settings
/// call it, else its file's name), where it is from, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unfit {
    pub name: String,
    /// From the settings (else `stockfish` found in `PATH`).
    pub from_settings: bool,
    pub why: Why,
}

/// The engine to analyse with, now.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    Ready(EngineLocation),
    /// An engine of the user's is being probed: asked again when it is done (the
    /// screen is repainted then).
    Checking,
    /// The user's engines do not do: the first of them.
    Unfit(Unfit),
    /// No engine at all.
    None,
}

/// The engine to analyse with, from what is known (nothing is probed here): ours, or a
/// user's engine already found to be the same.
pub fn engine() -> Option<EngineLocation> {
    #[cfg(not(web))]
    return crate::engines::own_stockfish().or_else(native::verified);
    #[cfg(web)]
    crate::engines::own_stockfish()
}

/// The engines of the user's to consider, in this order: the active one, the others of
/// the settings, `stockfish` from `PATH`. Called at the start, when the settings are
/// saved and when an analysis asks for an engine (not every frame).
#[cfg_attr(web, allow(unused_variables))]
pub fn set_candidates(settings: &crate::settings::Settings) {
    #[cfg(not(web))]
    native::set_candidates(native::candidates(settings));
}

/// The engine to analyse with; an engine of the user's not probed yet (or changed since)
/// is probed now, in the background.
#[cfg_attr(web, allow(unused_variables))]
pub fn lookup(ctx: &egui::Context) -> Lookup {
    #[cfg(not(web))]
    return native::lookup(ctx);
    #[cfg(web)]
    match engine() {
        Some(l) => Lookup::Ready(l),
        None => Lookup::None,
    }
}

/// Whether the engine at `path`, which said it is `name` at the start of a job, is the
/// one probed: ours always is; a user's one must still call itself Stockfish 19 (a
/// wrapper script may start another program than it did). One that does not is
/// forgotten, to be probed again.
#[cfg(not(web))]
pub fn name_fits(path: &std::path::Path, name: &str) -> bool {
    native::name_fits(path, name)
}

/// The program quits: the probes under way stop, their engines quit (or are killed).
pub fn shutdown() {
    #[cfg(not(web))]
    native::shutdown();
}

#[cfg(not(web))]
mod native {
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::thread::JoinHandle;

    use game_review::probe::{ProbeLimits, STOCKFISH_19, reference_key};
    use serde::{Deserialize, Serialize};

    use super::{Lookup, Unfit, Why};
    use crate::engines::EngineLocation;

    /// A probe's verdict on a file as it was.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub(super) struct Verdict {
        pub size: u64,
        pub modified: u64,
        pub same: bool,
        pub name: Option<String>,
    }

    /// What is kept on the disk: the verdicts and the reference they were made with.
    #[derive(Serialize, Deserialize)]
    pub(super) struct Kept {
        pub reference: String,
        pub engines: HashMap<PathBuf, Verdict>,
    }

    /// An engine of the user's: its file as given, the real file (links followed),
    /// what the settings call it, and whether it is from them.
    #[derive(Clone, Debug, PartialEq)]
    pub(super) struct Candidate {
        pub path: PathBuf,
        pub real: PathBuf,
        pub label: String,
        pub from_settings: bool,
    }

    impl Candidate {
        pub(super) fn new(path: PathBuf, label: String, from_settings: bool) -> Candidate {
            Candidate {
                real: path.canonicalize().unwrap_or_else(|_| path.clone()),
                path,
                label,
                from_settings,
            }
        }
    }

    #[derive(Default)]
    pub(super) struct State {
        pub candidates: Vec<Candidate>,
        /// By the real file.
        pub known: HashMap<PathBuf, Verdict>,
        /// Files that did not start, until it is said (by the real file).
        pub failed: HashSet<PathBuf>,
        pub checking: HashSet<PathBuf>,
        pub threads: Vec<JoinHandle<()>>,
        /// Where the verdicts are kept (`None`: nowhere).
        pub file: Option<PathBuf>,
        pub loaded: bool,
    }

    fn state() -> &'static Mutex<State> {
        static STATE: OnceLock<Mutex<State>> = OnceLock::new();
        STATE.get_or_init(Default::default)
    }

    /// Set when the program quits: the probes give up.
    static QUITTING: AtomicBool = AtomicBool::new(false);

    fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
        let mut s = state().lock().unwrap_or_else(|e| e.into_inner());
        if !s.loaded {
            s.loaded = true;
            s.file = crate::settings::Settings::cache_dir().map(|d| d.join("review-engines.json"));
            if let Some(bytes) = s.file.as_ref().and_then(|f| std::fs::read(f).ok()) {
                s.known = read_kept(&bytes);
            }
        }
        f(&mut s)
    }

    /// The verdicts kept, if they were made with the probe of this program.
    pub(super) fn read_kept(bytes: &[u8]) -> HashMap<PathBuf, Verdict> {
        serde_json::from_slice::<Kept>(bytes)
            .ok()
            .filter(|k| k.reference == reference_key())
            .map(|k| k.engines)
            .unwrap_or_default()
    }

    fn save(s: &State) {
        let Some(f) = &s.file else { return };
        let kept = Kept {
            reference: reference_key(),
            engines: s.known.clone(),
        };
        if let Ok(json) = serde_json::to_vec(&kept) {
            let _ = crate::platform::fs::write_atomic(f, &json);
        }
    }

    pub(super) fn candidates(settings: &crate::settings::Settings) -> Vec<Candidate> {
        let active = settings.active_engine.and_then(|i| settings.engines.get(i));
        let others = settings.engines.iter().filter(|e| Some(*e) != active);
        let mut out: Vec<Candidate> = active
            .into_iter()
            .chain(others)
            .filter(|e| !e.builtin)
            .map(|e| Candidate::new(e.path.clone(), e.name.clone(), true))
            .collect();
        if let Some(p) = crate::engines::system_stockfish() {
            let label = p.display().to_string();
            out.push(Candidate::new(p, label, false));
        }
        // The same file twice (the system's Stockfish added in the settings): once.
        let mut seen = HashSet::new();
        out.retain(|c| seen.insert(c.real.clone()));
        // The engines of the games against the computer (Rodent, Maia) are never
        // analysis engines, whatever the settings say: never probed.
        let play = play_dirs();
        out.retain(|c| !play.iter().any(|d| c.real.starts_with(d)));
        out
    }

    /// The folders of the engines ChessPenguin plays with (links followed).
    pub(super) fn play_dirs() -> Vec<PathBuf> {
        [
            crate::rodent_download::dir(),
            crate::rodent4_download::dir(),
            crate::maia_download::dir(),
        ]
        .into_iter()
        .flatten()
        .map(|d| d.canonicalize().unwrap_or(d))
        .collect()
    }

    pub(super) fn set_candidates(candidates: Vec<Candidate>) {
        with(|s| {
            if s.candidates != candidates {
                s.candidates = candidates;
            }
        });
    }

    /// The file's size and time (seconds), when it is there.
    pub(super) fn stamp(path: &Path) -> Option<(u64, u64)> {
        let m = std::fs::metadata(path).ok()?;
        if !m.is_file() {
            return None;
        }
        let modified = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        Some((m.len(), modified))
    }

    /// The verdict on the real file as it is now, if there is one.
    fn fresh<'a>(s: &'a State, real: &Path) -> Option<&'a Verdict> {
        let (size, modified) = stamp(real)?;
        s.known
            .get(real)
            .filter(|v| (v.size, v.modified) == (size, modified))
    }

    pub(super) fn verified() -> Option<EngineLocation> {
        with(|s| {
            s.candidates
                .iter()
                .find(|c| fresh(s, &c.real).is_some_and(|v| v.same))
                .map(|c| EngineLocation::Program(c.path.clone()))
        })
    }

    pub(super) fn lookup(ctx: &egui::Context) -> Lookup {
        match crate::engines::own_stockfish() {
            Some(own) => Lookup::Ready(own),
            None => lookup_without_own(ctx),
        }
    }

    /// [`lookup`] among the user's engines only.
    pub(super) fn lookup_without_own(ctx: &egui::Context) -> Lookup {
        match with(decide) {
            Decision::Done(l) => l,
            Decision::Probe(paths) => {
                for p in paths {
                    spawn_probe(p, ctx.clone());
                }
                Lookup::Checking
            }
        }
    }

    pub(super) enum Decision {
        Done(Lookup),
        /// These real files are to be probed (and are marked as being probed).
        Probe(Vec<PathBuf>),
    }

    /// What the verdicts say of the candidates; files without one are to be probed.
    /// The files that did not start are said once (then tried again).
    pub(super) fn decide(s: &mut State) -> Decision {
        let mut unfit = None;
        let mut checking = false;
        let mut to_probe = Vec::new();
        for c in &s.candidates {
            if stamp(&c.real).is_none() {
                continue;
            }
            let said = |name: Option<&String>, why| Unfit {
                name: name.cloned().unwrap_or_else(|| c.label.clone()),
                from_settings: c.from_settings,
                why,
            };
            if s.checking.contains(&c.real) {
                checking = true;
            } else if s.failed.contains(&c.real) {
                unfit.get_or_insert_with(|| said(None, Why::NotStarted));
            } else {
                match fresh(s, &c.real) {
                    Some(v) if v.same => {
                        let ready = EngineLocation::Program(c.path.clone());
                        return Decision::Done(Lookup::Ready(ready));
                    }
                    Some(v) => {
                        let why = if v.name.as_deref() == Some(STOCKFISH_19) {
                            Why::Differs
                        } else {
                            Why::Other
                        };
                        unfit.get_or_insert_with(|| said(v.name.as_ref(), why));
                    }
                    None => to_probe.push(c.real.clone()),
                }
            }
        }
        if !to_probe.is_empty() {
            s.checking.extend(to_probe.iter().cloned());
            return Decision::Probe(to_probe);
        }
        if checking {
            return Decision::Done(Lookup::Checking);
        }
        s.failed.clear();
        Decision::Done(unfit.map_or(Lookup::None, Lookup::Unfit))
    }

    /// Probes the real file on a thread of its own and keeps the verdict.
    fn spawn_probe(real: PathBuf, ctx: egui::Context) {
        let spawned = std::thread::Builder::new()
            .name("review-engine-probe".into())
            .spawn({
                let real = real.clone();
                move || {
                    let stamp = stamp(&real);
                    let config = uci::EngineConfig {
                        nice: 10,
                        ..uci::EngineConfig::new(&real)
                    };
                    let quitting = || QUITTING.load(Ordering::Relaxed);
                    let probe =
                        game_review::probe::probe(config, ProbeLimits::default(), &quitting);
                    if !quitting() {
                        record(&real, stamp, probe);
                        ctx.request_repaint();
                    }
                }
            });
        match spawned {
            Ok(t) => with(|s| {
                s.threads.retain(|t| !t.is_finished());
                s.threads.push(t);
            }),
            Err(e) => record(
                &real,
                stamp(&real),
                Err(game_review::JobFailure::Start(e.to_string())),
            ),
        }
    }

    pub(super) fn record(
        real: &Path,
        stamp: Option<(u64, u64)>,
        probe: Result<game_review::probe::Probe, game_review::JobFailure>,
    ) {
        with(|s| {
            s.checking.remove(real);
            let Some((size, modified)) = stamp else {
                return;
            };
            match probe {
                Ok(p) => {
                    s.known.insert(
                        real.to_path_buf(),
                        Verdict {
                            size,
                            modified,
                            same: p.same(),
                            name: p.name,
                        },
                    );
                    save(s);
                }
                // Not remembered: tried again at the next request.
                Err(_) => {
                    s.failed.insert(real.to_path_buf());
                }
            }
        });
    }

    pub(super) fn name_fits(path: &Path, name: &str) -> bool {
        let own = crate::engines::own_stockfish()
            .is_some_and(|EngineLocation::Program(p)| p.as_path() == path);
        if own || name == STOCKFISH_19 {
            return true;
        }
        let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        with(|s| {
            if s.known.remove(&real).is_some() {
                save(s);
            }
        });
        false
    }

    pub(super) fn shutdown() {
        QUITTING.store(true, Ordering::Relaxed);
        let threads = with(|s| std::mem::take(&mut s.threads));
        // Each gives up within a tenth of a second, and its engine quits or is killed
        // within the `uci` crate's stop and quit timeouts.
        for t in threads {
            let _ = t.join();
        }
    }

    #[cfg(test)]
    pub(super) fn reset(candidates: Vec<Candidate>) {
        let mut s = state().lock().unwrap_or_else(|e| e.into_inner());
        *s = State {
            candidates,
            loaded: true,
            ..State::default()
        };
    }
}

#[cfg(all(test, not(web)))]
mod tests {
    use super::native::{Candidate, Kept, Verdict, lookup_without_own, read_kept, reset, verified};
    use super::{EngineLocation, Lookup, Unfit, Why, name_fits};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    fn fake() -> PathBuf {
        let fake = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/debug/fake_uci"
        ));
        assert!(
            fake.exists(),
            "build the fake engine first: cargo build -p uci --bin fake_uci"
        );
        fake
    }

    /// A script starting the fake engine named `name` that answers the probe with the
    /// reference, `nodes_delta` nodes more.
    fn engine(dir: &Path, file: &str, name: &str, nodes_delta: u64) -> PathBuf {
        use game_review::probe::{PROBE_POSITIONS, REFERENCE};
        let mut args = vec![format!("--name '{name}'")];
        for (fen, (best, score, nodes)) in PROBE_POSITIONS.iter().zip(REFERENCE) {
            let uci::Score::Cp(cp) = score else {
                unreachable!("the reference scores are in centipawns")
            };
            let marker = fen.split(' ').next().unwrap_or_default();
            args.push(format!(
                "--answer '{marker}' 'cp {cp} nodes {} pv {best}'",
                nodes + nodes_delta
            ));
        }
        let path = dir.join(file);
        let script = format!(
            "#!/bin/sh\nexec '{}' {}\n",
            fake().display(),
            args.join(" ")
        );
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    fn settle(ctx: &egui::Context) -> Lookup {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let l = lookup_without_own(ctx);
            if l != Lookup::Checking || Instant::now() > deadline {
                return l;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn unfit(name: &str, from_settings: bool, why: Why) -> Lookup {
        Lookup::Unfit(Unfit {
            name: name.into(),
            from_settings,
            why,
        })
    }

    /// One test: the state is global.
    #[test]
    fn a_users_stockfish_19_is_taken_only_when_it_searches_as_ours() {
        let dir = std::env::temp_dir().join(format!("cp-review-engine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();
        let differs = engine(&dir, "sf-other", "Stockfish 19", 1);
        let same = engine(&dir, "sf-same", "Stockfish 19", 0);
        let named = engine(&dir, "sf-18", "Stockfish 18", 0);
        let settings = |p: &PathBuf, label: &str| Candidate::new(p.clone(), label.into(), true);

        // No engine of the user's.
        reset(Vec::new());
        assert_eq!(settle(&ctx), Lookup::None);

        // A Stockfish 19 searching otherwise (one node more): not taken.
        reset(vec![settings(&differs, "My engine")]);
        assert_eq!(lookup_without_own(&ctx), Lookup::Checking);
        assert_eq!(settle(&ctx), unfit("Stockfish 19", true, Why::Differs));
        assert_eq!(verified(), None);

        // Another name: turned down (without a search); from `PATH` it says so.
        reset(vec![Candidate::new(
            named.clone(),
            "/usr/bin/x".into(),
            false,
        )]);
        assert_eq!(settle(&ctx), unfit("Stockfish 18", false, Why::Other));

        // An engine that does not start: its name from the settings; not remembered,
        // so the next request tries it again.
        let broken = dir.join("broken");
        std::fs::write(&broken, "not a program").unwrap();
        reset(vec![settings(&broken, "Broken one")]);
        assert_eq!(settle(&ctx), unfit("Broken one", true, Why::NotStarted));
        assert_eq!(lookup_without_own(&ctx), Lookup::Checking);
        assert_eq!(settle(&ctx), unfit("Broken one", true, Why::NotStarted));

        // The reference answers: taken, even after another engine in the list.
        reset(vec![settings(&differs, "A"), settings(&same, "B")]);
        let ready = Lookup::Ready(EngineLocation::Program(same.clone()));
        assert_eq!(settle(&ctx), ready);
        assert_eq!(verified(), Some(EngineLocation::Program(same.clone())));
        // Remembered: no second probe.
        assert_eq!(lookup_without_own(&ctx), ready);

        // A link to it: the real file's verdict, nothing probed again.
        #[cfg(unix)]
        {
            let link = dir.join("link-to-same");
            std::os::unix::fs::symlink(&same, &link).unwrap();
            super::native::set_candidates(vec![settings(&link, "Link")]);
            assert_eq!(
                lookup_without_own(&ctx),
                Lookup::Ready(EngineLocation::Program(link.clone()))
            );
            super::native::set_candidates(vec![settings(&same, "B")]);
        }

        // At a job's start it says another name (a wrapper changed): forgotten.
        assert!(name_fits(&same, "Stockfish 19"));
        assert!(!name_fits(&same, "Stockfish 18"));
        assert_eq!(verified(), None);
        assert_eq!(settle(&ctx), ready);

        // The file changed: probed again (a changed size).
        let text = std::fs::read_to_string(&same).unwrap();
        std::fs::write(&same, format!("{text}\n")).unwrap();
        assert_eq!(verified(), None);
        assert_eq!(lookup_without_own(&ctx), Lookup::Checking);
        assert_eq!(settle(&ctx), ready);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The engines of the games against the computer are never candidates, whatever
    /// the settings say; the analysis engines are, the active one first.
    #[test]
    fn play_engines_are_never_probed() {
        use crate::settings::{EngineConfig, Settings};
        let rodent = crate::rodent_download::dir()
            .expect("a data folder")
            .join("rodent_v");
        let rodent4 = crate::rodent4_download::dir()
            .expect("a data folder")
            .join("rodent4");
        let mine = PathBuf::from("/opt/engines/stockfish-19");
        let other = PathBuf::from("/opt/engines/komodo");
        let settings = Settings {
            engines: vec![
                EngineConfig::new("Rodent".into(), rodent),
                EngineConfig::new("Komodo".into(), other.clone()),
                EngineConfig::new("Rodent IV".into(), rodent4),
                EngineConfig::new("Mine".into(), mine.clone()),
            ],
            active_engine: Some(3),
            ..Settings::default()
        };
        let found: Vec<(PathBuf, bool)> = super::native::candidates(&settings)
            .into_iter()
            .map(|c| (c.path, c.from_settings))
            .filter(|(_, from_settings)| *from_settings)
            .collect();
        assert_eq!(found, vec![(mine, true), (other, true)]);
    }

    /// Verdicts kept with another reference (another probe) are not trusted.
    #[test]
    fn verdicts_of_another_reference_are_dropped() {
        let verdict = Verdict {
            size: 1,
            modified: 2,
            same: true,
            name: Some("Stockfish 19".into()),
        };
        let kept = |reference: String| {
            serde_json::to_vec(&Kept {
                reference,
                engines: [(PathBuf::from("/x/stockfish"), verdict.clone())].into(),
            })
            .unwrap()
        };
        let ours = read_kept(&kept(game_review::probe::reference_key()));
        assert_eq!(ours.get(Path::new("/x/stockfish")), Some(&verdict));
        assert!(read_kept(&kept("0000000000000000".into())).is_empty());
        // The first version's file (a bare map): dropped too.
        assert!(
            read_kept(br#"{"/x/stockfish":{"size":1,"modified":2,"same":true,"name":null}}"#)
                .is_empty()
        );
    }
}
