//! Numeric Annotation Glyphs.

use std::fmt;

/// A PGN NAG (`$n`). Unknown codes are preserved and shown as `$n`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Nag(pub u8);

impl Nag {
    pub const GOOD_MOVE: Nag = Nag(1);
    pub const MISTAKE: Nag = Nag(2);
    pub const BRILLIANT_MOVE: Nag = Nag(3);
    pub const BLUNDER: Nag = Nag(4);
    pub const SPECULATIVE_MOVE: Nag = Nag(5);
    pub const DUBIOUS_MOVE: Nag = Nag(6);

    /// Display symbol, if a conventional one exists.
    pub fn symbol(self) -> Option<&'static str> {
        Some(match self.0 {
            1 => "!",
            2 => "?",
            3 => "!!",
            4 => "??",
            5 => "!?",
            6 => "?!",
            7 | 8 => "□",
            10..=12 => "=",
            13 => "∞",
            14 => "⩲",
            15 => "⩱",
            16 => "±",
            17 => "∓",
            18 => "+−",
            19 => "−+",
            22 | 23 => "⨀",
            32 | 33 => "⟳",
            36 | 37 => "↑",
            40 | 41 => "→",
            44 | 45 => "=/∞",
            132 | 133 => "⇆",
            138 | 139 => "⊕",
            140 => "∆",
            142 => "⌓",
            145 => "RR",
            146 => "N",
            _ => return None,
        })
    }

    /// Move-quality glyphs ($1..$6) are shown glued to the move; others separated.
    pub fn is_move_assessment(self) -> bool {
        (1..=6).contains(&self.0)
    }

    /// Parses a symbolic annotation as it may appear in PGN import (`!?`, `+-`, ...).
    pub fn from_symbol(s: &str) -> Option<Nag> {
        Some(Nag(match s {
            "!" => 1,
            "?" => 2,
            "!!" => 3,
            "??" => 4,
            "!?" => 5,
            "?!" => 6,
            "□" => 7,
            "=" => 10,
            "∞" => 13,
            "+=" | "+/=" | "⩲" => 14,
            "=+" | "=/+" | "⩱" => 15,
            "+/-" | "±" => 16,
            "-/+" | "∓" => 17,
            "+-" | "+−" => 18,
            "-+" | "−+" => 19,
            _ => return None,
        }))
    }
}

impl fmt::Display for Nag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.symbol() {
            Some(s) => f.write_str(s),
            None => write!(f, "${}", self.0),
        }
    }
}
