//! The engine of the game analysis and the search for opening errors: ChessPenguin's
//! Stockfish 19, or else a Stockfish 19 of the user's (from the settings, or the one in
//! `PATH`) that searches exactly as ours (decision of 2026-10-31). Whether it does is
//! told by short searches of the analysis's fixed mode (`game_review::probe`), made on
//! a thread of their own; the verdict is remembered with the file's size and time, so
//! a file is probed once, and again only when it changes.

use crate::engines::EngineLocation;

/// The engine to analyse with, now.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    Ready(EngineLocation),
    /// An engine of the user's is being probed: asked again when it is done (the
    /// screen is repainted then).
    Checking,
    /// The user's engines are other ones: the name of the first (its `id name`, or
    /// what the settings call it).
    Other(String),
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
/// the settings, `stockfish` from `PATH`. Called at the start and when the settings
/// are saved.
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

#[cfg(not(web))]
mod native {
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};

    use serde::{Deserialize, Serialize};

    use super::Lookup;
    use crate::engines::EngineLocation;

    /// A probe's verdict on a file as it was.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub(super) struct Verdict {
        pub size: u64,
        pub modified: u64,
        pub same: bool,
        pub name: Option<String>,
        /// The engine did not start or answer: tried again at the next start (never
        /// kept on the disk).
        #[serde(skip)]
        pub failed: bool,
    }

    /// An engine of the user's: its file and what the settings call it.
    #[derive(Clone, Debug, PartialEq)]
    pub(super) struct Candidate {
        pub path: PathBuf,
        pub label: String,
    }

    #[derive(Default)]
    pub(super) struct State {
        pub candidates: Vec<Candidate>,
        pub known: HashMap<PathBuf, Verdict>,
        pub checking: HashSet<PathBuf>,
        /// Where the verdicts are kept (`None`: not read yet, or nowhere).
        pub file: Option<PathBuf>,
        pub loaded: bool,
    }

    fn state() -> &'static Mutex<State> {
        static STATE: OnceLock<Mutex<State>> = OnceLock::new();
        STATE.get_or_init(Default::default)
    }

    fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
        let mut s = state().lock().unwrap_or_else(|e| e.into_inner());
        if !s.loaded {
            s.loaded = true;
            s.file = crate::settings::Settings::cache_dir().map(|d| d.join("review-engines.json"));
            if let Some(known) = s
                .file
                .as_ref()
                .and_then(|f| std::fs::read(f).ok())
                .and_then(|b| serde_json::from_slice(&b).ok())
            {
                s.known = known;
            }
        }
        f(&mut s)
    }

    pub(super) fn candidates(settings: &crate::settings::Settings) -> Vec<Candidate> {
        let active = settings.active_engine.and_then(|i| settings.engines.get(i));
        let others = settings.engines.iter().filter(|e| Some(*e) != active);
        let mut out: Vec<Candidate> = active
            .into_iter()
            .chain(others)
            .filter(|e| !e.builtin)
            .map(|e| Candidate {
                path: e.path.clone(),
                label: e.name.clone(),
            })
            .collect();
        if let Some(p) = crate::engines::system_stockfish() {
            out.push(Candidate {
                label: p
                    .file_name()
                    .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into()),
                path: p,
            });
        }
        // The same file twice (the system's Stockfish added in the settings): once.
        let mut seen = HashSet::new();
        out.retain(|c| seen.insert(c.path.canonicalize().unwrap_or_else(|_| c.path.clone())));
        out
    }

    pub(super) fn set_candidates(candidates: Vec<Candidate>) {
        with(|s| s.candidates = candidates);
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

    /// The verdict on the file as it is now, if there is one.
    fn fresh<'a>(s: &'a State, path: &Path) -> Option<&'a Verdict> {
        let (size, modified) = stamp(path)?;
        s.known
            .get(path)
            .filter(|v| (v.size, v.modified) == (size, modified))
    }

    pub(super) fn verified() -> Option<EngineLocation> {
        with(|s| {
            s.candidates
                .iter()
                .find(|c| fresh(s, &c.path).is_some_and(|v| v.same))
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
        let to_probe = with(decide);
        match to_probe {
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
        /// These files are to be probed (and are marked as being probed).
        Probe(Vec<PathBuf>),
    }

    /// What the verdicts say of the candidates; files without one are to be probed.
    pub(super) fn decide(s: &mut State) -> Decision {
        let mut other = None;
        let mut checking = false;
        let mut to_probe = Vec::new();
        for c in &s.candidates {
            if stamp(&c.path).is_none() {
                continue;
            }
            match fresh(s, &c.path) {
                Some(v) if v.same => {
                    return Decision::Done(Lookup::Ready(EngineLocation::Program(c.path.clone())));
                }
                Some(v) => {
                    other.get_or_insert_with(|| v.name.clone().unwrap_or_else(|| c.label.clone()));
                }
                None if s.checking.contains(&c.path) => checking = true,
                None => to_probe.push(c.path.clone()),
            }
        }
        if !to_probe.is_empty() {
            s.checking.extend(to_probe.iter().cloned());
            return Decision::Probe(to_probe);
        }
        if checking {
            return Decision::Done(Lookup::Checking);
        }
        Decision::Done(other.map_or(Lookup::None, Lookup::Other))
    }

    /// Probes the file on a thread of its own and keeps the verdict.
    fn spawn_probe(path: PathBuf, ctx: egui::Context) {
        let spawned = std::thread::Builder::new()
            .name("review-engine-probe".into())
            .spawn({
                let path = path.clone();
                move || {
                    let stamp = stamp(&path);
                    let config = uci::EngineConfig {
                        nice: 10,
                        ..uci::EngineConfig::new(&path)
                    };
                    let probe = game_review::probe::probe(config);
                    record(&path, stamp, probe);
                    ctx.request_repaint();
                }
            });
        if spawned.is_err() {
            record(
                &path,
                stamp(&path),
                Err(game_review::JobFailure::Start("no thread".into())),
            );
        }
    }

    pub(super) fn record(
        path: &Path,
        stamp: Option<(u64, u64)>,
        probe: Result<game_review::probe::Probe, game_review::JobFailure>,
    ) {
        with(|s| {
            s.checking.remove(path);
            let Some((size, modified)) = stamp else {
                return;
            };
            let verdict = match probe {
                Ok(p) => Verdict {
                    size,
                    modified,
                    same: p.same(),
                    name: p.name,
                    failed: false,
                },
                Err(_) => Verdict {
                    size,
                    modified,
                    same: false,
                    name: None,
                    failed: true,
                },
            };
            s.known.insert(path.to_path_buf(), verdict);
            if let Some(f) = &s.file {
                let kept: HashMap<&PathBuf, &Verdict> =
                    s.known.iter().filter(|(_, v)| !v.failed).collect();
                if let Ok(json) = serde_json::to_vec(&kept) {
                    let _ = crate::platform::fs::write_atomic(f, &json);
                }
            }
        });
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
    use super::native::*;
    use super::*;
    use std::path::PathBuf;
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

    /// A script starting the fake engine that answers the probe as `answers` say.
    fn engine(dir: &std::path::Path, file: &str, name: &str, nodes_delta: u64) -> PathBuf {
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
            let l = native::lookup_without_own(ctx);
            if l != Lookup::Checking || Instant::now() > deadline {
                return l;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// One test: the state is global.
    #[test]
    fn a_users_stockfish_19_is_taken_only_when_it_searches_as_ours() {
        let dir = std::env::temp_dir().join(format!("cp-review-engine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();
        let other = engine(&dir, "sf-other", "Stockfish 19", 1);
        let same = engine(&dir, "sf-same", "Stockfish 19", 0);
        let named = engine(&dir, "sf-18", "Stockfish 18", 0);
        let candidate = |p: &PathBuf, label: &str| Candidate {
            path: p.clone(),
            label: label.into(),
        };

        // No engine of the user's.
        reset(Vec::new());
        assert_eq!(settle(&ctx), Lookup::None);

        // Another search (one node more): not taken; its name is told.
        reset(vec![candidate(&other, "My engine")]);
        assert_eq!(native::lookup_without_own(&ctx), Lookup::Checking);
        assert_eq!(settle(&ctx), Lookup::Other("Stockfish 19".into()));
        assert_eq!(native::verified(), None);

        // The same answers under another name: not taken either.
        reset(vec![candidate(&named, "SF")]);
        assert_eq!(settle(&ctx), Lookup::Other("Stockfish 18".into()));

        // An engine that does not start: its name from the settings.
        let missing = dir.join("broken");
        std::fs::write(&missing, "not a program").unwrap();
        reset(vec![candidate(&missing, "Broken one")]);
        assert_eq!(settle(&ctx), Lookup::Other("Broken one".into()));

        // The reference answers: taken, even after another engine in the list.
        reset(vec![candidate(&other, "A"), candidate(&same, "B")]);
        assert_eq!(
            settle(&ctx),
            Lookup::Ready(EngineLocation::Program(same.clone()))
        );
        assert_eq!(
            native::verified(),
            Some(EngineLocation::Program(same.clone()))
        );
        // Remembered: no second probe.
        assert_eq!(
            native::lookup_without_own(&ctx),
            Lookup::Ready(EngineLocation::Program(same.clone()))
        );

        // The file changed: probed again (a changed size).
        let text = std::fs::read_to_string(&same).unwrap();
        std::fs::write(&same, format!("{text}\n")).unwrap();
        assert_eq!(native::verified(), None);
        assert_eq!(native::lookup_without_own(&ctx), Lookup::Checking);
        assert_eq!(
            settle(&ctx),
            Lookup::Ready(EngineLocation::Program(same.clone()))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
