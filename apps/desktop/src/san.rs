//! How moves look on screen (docs/design/move-notation.md): Latin letters, figurines as
//! on Lichess, or the piece letters of the interface language. Only the shown label
//! changes — the game, PGN, the clipboard and everything compared stay Latin SAN.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Deserializer, Serialize};

use crate::i18n::Lang;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MoveNotation {
    /// K Q R B N, as in PGN files and engine output.
    #[default]
    Latin,
    /// ♔ ♕ ♖ ♗ ♘ (one outline style for both colours, no pawn sign).
    Figurine,
    /// The letters of the interface language (Кр Ф Л С К in Russian); Latin in English.
    Local,
}

/// An unknown value (a newer version) is read as Latin, not as broken settings.
impl<'de> Deserialize<'de> for MoveNotation {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Ok(match v.as_str().unwrap_or("") {
            "figurine" => MoveNotation::Figurine,
            "local" => MoveNotation::Local,
            _ => MoveNotation::Latin,
        })
    }
}

static MODE: AtomicU8 = AtomicU8::new(0);

/// The notation chosen in the settings (set by the application every frame).
pub fn set(mode: MoveNotation) {
    MODE.store(mode as u8, Ordering::Relaxed);
}

fn mode() -> MoveNotation {
    match MODE.load(Ordering::Relaxed) {
        1 => MoveNotation::Figurine,
        2 => MoveNotation::Local,
        _ => MoveNotation::Latin,
    }
}

/// King, queen, rook, bishop, knight in a language (`None`: as Latin).
pub fn local_letters(lang: Lang) -> Option<[&'static str; 5]> {
    match lang {
        Lang::En => None,
        Lang::Ru => Some(["Кр", "Ф", "Л", "С", "К"]),
        Lang::De => Some(["K", "D", "T", "L", "S"]),
        Lang::Fr => Some(["R", "D", "T", "F", "C"]),
        Lang::Es | Lang::It => Some(["R", "D", "T", "A", "C"]),
    }
}

fn piece(c: char, mode: MoveNotation, lang: Lang) -> Option<&'static str> {
    let i = "KQRBN".find(c)?;
    match mode {
        MoveNotation::Latin => None,
        MoveNotation::Figurine => Some(["♔", "♕", "♖", "♗", "♘"][i]),
        MoveNotation::Local => local_letters(lang).map(|l| l[i]),
    }
}

/// A SAN move as shown: the piece letter at the start and after "=" replaced.
pub fn display_san(san: &str, mode: MoveNotation, lang: Lang) -> Cow<'_, str> {
    if mode == MoveNotation::Latin || (mode == MoveNotation::Local && lang == Lang::En) {
        return Cow::Borrowed(san);
    }
    let mut out = String::with_capacity(san.len() + 4);
    let mut first = true;
    let mut after_eq = false;
    let mut changed = false;
    for c in san.chars() {
        match (first || after_eq).then(|| piece(c, mode, lang)).flatten() {
            Some(p) => {
                out.push_str(p);
                changed = true;
            }
            None => out.push(c),
        }
        first = false;
        after_eq = c == '=';
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(san)
    }
}

/// A label as shown: "12.Nf3", "12…Nf3", "12...Nf3" or "Nf3", in the chosen notation.
pub fn show(label: &str) -> Cow<'_, str> {
    show_in(label, mode(), crate::i18n::current())
}

/// A line of labels separated by spaces ("5.Nc3 a6 6.Be2"), as shown.
pub fn show_line(line: &str) -> String {
    line.split(' ')
        .map(|w| show(w))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn show_in(label: &str, mode: MoveNotation, lang: Lang) -> Cow<'_, str> {
    let number = label
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || matches!(c, '.' | '…' | ' ')))
        .map_or(label.len(), |(i, _)| i);
    let (prefix, san) = label.split_at(number);
    match display_san(san, mode, lang) {
        Cow::Borrowed(_) => Cow::Borrowed(label),
        Cow::Owned(s) => Cow::Owned(format!("{prefix}{s}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_piece_letters_change() {
        let f = |s| display_san(s, MoveNotation::Figurine, Lang::Ru).into_owned();
        assert_eq!(f("Nf3"), "♘f3");
        assert_eq!(f("Qxd8+"), "♕xd8+");
        assert_eq!(f("exd8=Q#"), "exd8=♕#");
        assert_eq!(f("e8=N"), "e8=♘");
        assert_eq!(f("O-O-O"), "O-O-O");
        assert_eq!(f("--"), "--");
        assert_eq!(f("Nbd7"), "♘bd7");
        assert_eq!(f("Rxb7"), "♖xb7");
        assert_eq!(f("e4"), "e4");
        let r = |s| display_san(s, MoveNotation::Local, Lang::Ru).into_owned();
        assert_eq!(r("Kxe2+"), "Крxe2+");
        assert_eq!(r("Bb5"), "Сb5");
        assert_eq!(r("a8=Q"), "a8=Ф");
        let de = |s| display_san(s, MoveNotation::Local, Lang::De).into_owned();
        assert_eq!(de("Nf3"), "Sf3");
        assert_eq!(de("Qd1"), "Dd1");
        let fr = |s| display_san(s, MoveNotation::Local, Lang::Fr).into_owned();
        assert_eq!(fr("Kg1"), "Rg1");
        assert_eq!(fr("Rg1"), "Tg1");
        // Applied once, to Latin: a shown label is never shown again.
        assert_eq!(show_in("Kg1", MoveNotation::Local, Lang::Fr), "Rg1");
    }

    #[test]
    fn latin_and_english_borrow() {
        assert!(matches!(
            display_san("Nf3", MoveNotation::Latin, Lang::Ru),
            Cow::Borrowed(_)
        ));
        assert!(matches!(
            display_san("Nf3", MoveNotation::Local, Lang::En),
            Cow::Borrowed(_)
        ));
        assert!(matches!(
            display_san("e4", MoveNotation::Figurine, Lang::Ru),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn labels_keep_their_numbers() {
        let s = |l| show_in(l, MoveNotation::Figurine, Lang::En).into_owned();
        assert_eq!(s("12.Nf3"), "12.♘f3");
        assert_eq!(s("12…Nf6"), "12…♘f6");
        assert_eq!(s("12...Bb4+"), "12...♗b4+");
        assert_eq!(s("Kh1"), "♔h1");
        assert_eq!(s("3.e4"), "3.e4");
        assert_eq!(s("12. Nf3"), "12. ♘f3");
        assert_eq!(s("12... Nf6"), "12... ♘f6");
    }

    #[test]
    fn settings_values() {
        let v: MoveNotation = serde_json::from_str("\"figurine\"").unwrap();
        assert_eq!(v, MoveNotation::Figurine);
        let v: MoveNotation = serde_json::from_str("\"klingon\"").unwrap();
        assert_eq!(v, MoveNotation::Latin);
        assert_eq!(
            serde_json::to_string(&MoveNotation::Local).unwrap(),
            "\"local\""
        );
    }
}
