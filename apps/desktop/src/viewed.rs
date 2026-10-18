//! Games the user has opened, remembered by their content (so a mark follows the game
//! when a file is re-sorted or other games are added) in
//! `$XDG_DATA_HOME/chesspenguin/viewed.json`.

use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Default)]
pub struct Viewed {
    games: HashSet<u64>,
}

impl Viewed {
    fn file() -> Option<PathBuf> {
        directories::ProjectDirs::from("io.github", "vrubelroman", "ChessPenguin")
            .map(|d| d.data_dir().join("viewed.json"))
    }

    pub fn load() -> Viewed {
        let games = Self::file()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Vec<u64>>(&b).ok())
            .unwrap_or_default();
        Viewed {
            games: games.into_iter().collect(),
        }
    }

    fn save(&self) {
        let Some(path) = Self::file() else { return };
        let mut list: Vec<u64> = self.games.iter().copied().collect();
        list.sort_unstable();
        match serde_json::to_vec(&list) {
            Ok(bytes) => {
                if let Err(e) = crate::settings::write_atomic(&path, &bytes) {
                    eprintln!("viewed games: {}: {e}", path.display());
                }
            }
            Err(e) => eprintln!("viewed games: {e}"),
        }
    }

    pub fn contains(&self, key: u64) -> bool {
        self.games.contains(&key)
    }

    pub fn set(&mut self, key: u64, viewed: bool) {
        let changed = if viewed {
            self.games.insert(key)
        } else {
            self.games.remove(&key)
        };
        if changed {
            self.save();
        }
    }
}

/// Identity of a game: FNV-1a 64 of its PGN text without surrounding whitespace.
pub fn key(raw: &[u8]) -> u64 {
    let start = raw
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(raw.len());
    let end = raw
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |i| i + 1);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in &raw[start..end.max(start)] {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    #[test]
    fn key_ignores_surrounding_whitespace() {
        assert_eq!(
            super::key(b"\n[Event \"x\"]\n1. e4 *\n\n"),
            super::key(b"[Event \"x\"]\n1. e4 *")
        );
        assert_ne!(super::key(b"1. e4 *"), super::key(b"1. d4 *"));
        assert_eq!(super::key(b"   "), super::key(b""));
    }
}
