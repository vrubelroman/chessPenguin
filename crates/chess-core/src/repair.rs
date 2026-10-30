//! Compatibility repair of letters replaced by ASCII sequences in the source.
//!
//! Observed in the Dvoretsky textbook (windows-1251): the letters «ч», «Ч» and
//! «ю» never occur; instead comments contain `~~` (ч), `><` (Ч) and `o^`
//! (ю, with a Latin `o`). The original bytes are never changed; the repair is
//! applied only to comment text of parsed games, only when detected for the
//! whole source, and can be switched off.

use crate::encoding::TextEncoding;

/// Replacement table: (sequence, letter). Longer/specific sequences first.
const TABLE: [(&str, char); 6] = [
    ("><", 'Ч'),
    ("~~", 'ч'),
    ("O^", 'Ю'),
    ("О^", 'Ю'), // Cyrillic О
    ("o^", 'ю'),
    ("о^", 'ю'), // Cyrillic о
];

/// Counts of restored letters.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct LetterRepairCounts {
    /// `~~` → «ч».
    pub ch_lower: usize,
    /// `><` → «Ч», plus `~~` at the start of a sentence/comment → «Ч».
    pub ch_upper: usize,
    /// `o^`/`о^` → «ю».
    pub yu_lower: usize,
    /// `O^`/`О^` → «Ю».
    pub yu_upper: usize,
}

impl LetterRepairCounts {
    pub fn total(&self) -> usize {
        self.ch_lower + self.ch_upper + self.yu_lower + self.yu_upper
    }

    pub fn add(&mut self, o: &LetterRepairCounts) {
        self.ch_lower += o.ch_lower;
        self.ch_upper += o.ch_upper;
        self.yu_lower += o.yu_lower;
        self.yu_upper += o.yu_upper;
    }
}

/// Source-level detection result.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct LetterRepairDetection {
    /// Substitution pattern found (see [`detect_letter_substitution`]).
    pub detected: bool,
    /// Sequences found in the whole source (before repair).
    pub sequences: usize,
    /// Genuine «ч», «Ч», «ю», «Ю» found in the whole source.
    pub genuine_letters: usize,
}

/// Minimum number of substitution sequences to consider the pattern real.
const MIN_SEQUENCES: usize = 20;

/// Detects the substitution for a whole source.
///
/// Rule: the encoding is a legacy Cyrillic code page (windows-1251, KOI8-R,
/// IBM866); at least 20 sequences from the table occur directly next to a
/// Cyrillic letter (e.g. `и~~ь`, `{><е`, `o^т`); genuine «ч/Ч/ю/Ю» occur less
/// than once per 50 sequences (in the textbook: 0); and at least 90 % of all
/// `~~`/`^` occurrences are such sequences (ASCII art or real tildes elsewhere
/// disable the repair).
pub fn detect_letter_substitution(bytes: &[u8], encoding: TextEncoding) -> LetterRepairDetection {
    if !matches!(
        encoding,
        TextEncoding::Windows1251 | TextEncoding::Koi8R | TextEncoding::Ibm866
    ) {
        return LetterRepairDetection::default();
    }
    // The source's letters as they come, never all at once: a large file's text is
    // twice its size in UTF-8, four times as `char`s (more than a browser's memory).
    let chars = || source_chars(bytes, encoding);
    let genuine_letters = chars()
        .filter(|c| matches!(c, 'ч' | 'Ч' | 'ю' | 'Ю'))
        .count();
    let sequences = sequences(chars());
    let markers = chars().filter(|c| matches!(c, '~' | '^')).count();
    // Each `~~` has two markers, each `x^` one; `><` has none.
    let detected = sequences >= MIN_SEQUENCES
        && genuine_letters * 50 < sequences
        && markers * 9 <= sequence_markers(chars()) * 10;
    LetterRepairDetection {
        detected,
        sequences,
        genuine_letters,
    }
}

/// The letters of `bytes` in a single-byte `encoding`, decoded a piece at a time (a
/// byte is a letter: any place splits the text).
fn source_chars(bytes: &[u8], encoding: TextEncoding) -> impl Iterator<Item = char> + '_ {
    bytes.chunks(1 << 20).flat_map(move |piece| {
        let (text, _) = encoding.decode(piece);
        text.chars().collect::<Vec<char>>()
    })
}

/// The sequences of the table next to a Cyrillic letter (a pair taken is not looked at
/// again).
fn sequences(chars: impl Iterator<Item = char>) -> usize {
    let is_cyr =
        |c: Option<char>| c.is_some_and(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё'));
    let mut chars = chars.peekable();
    let mut n = 0;
    let mut before = None;
    while let Some(a) = chars.next() {
        let Some(&b) = chars.peek() else { break };
        if TABLE.iter().any(|(s, _)| s.chars().eq([a, b])) {
            chars.next();
            if is_cyr(before) || is_cyr(chars.peek().copied()) {
                n += 1;
            }
            before = Some(b);
        } else {
            before = Some(a);
        }
    }
    n
}

fn sequence_markers(chars: impl Iterator<Item = char>) -> usize {
    let mut chars = chars.peekable();
    let mut n = 0;
    while let Some(a) = chars.next() {
        let Some(&b) = chars.peek() else { break };
        match (a, b) {
            ('~', '~') => {
                n += 2;
                chars.next();
            }
            ('o' | 'о' | 'O' | 'О', '^') => {
                n += 1;
                chars.next();
            }
            _ => {}
        }
    }
    n
}

/// Restores letters in one text. `~~` at the start of the text or of a
/// sentence (after `.`, `!`, `?` and whitespace) becomes «Ч».
pub fn repair_letters(text: &str) -> (String, LetterRepairCounts) {
    let mut counts = LetterRepairCounts::default();
    if !text.contains(['~', '^', '<']) {
        return (text.to_owned(), counts);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let hit = TABLE.iter().find(|(s, _)| rest.starts_with(s));
        match hit {
            Some((seq, letter)) => {
                let letter = if *seq == "~~" && at_sentence_start(&out) {
                    counts.ch_upper += 1;
                    'Ч'
                } else {
                    match letter {
                        'Ч' => counts.ch_upper += 1,
                        'ч' => counts.ch_lower += 1,
                        'Ю' => counts.yu_upper += 1,
                        _ => counts.yu_lower += 1,
                    }
                    *letter
                };
                out.push(letter);
                rest = &rest[seq.len()..];
            }
            None => {
                let mut it = rest.chars();
                if let Some(c) = it.next() {
                    out.push(c);
                }
                rest = it.as_str();
            }
        }
    }
    (out, counts)
}

fn at_sentence_start(before: &str) -> bool {
    let trimmed = before.trim_end();
    if trimmed.is_empty() {
        return true;
    }
    // Require whitespace between the punctuation and the word.
    trimmed.len() < before.len() && trimmed.ends_with(['.', '!', '?'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repairs_table() {
        let (s, c) = repair_letters("><ерные играo^т. ~~то делать ~~ерным? Оппозицио^ O^г");
        assert_eq!(s, "Черные играют. Что делать черным? Оппозицию Юг");
        assert_eq!(
            c,
            LetterRepairCounts {
                ch_lower: 1,
                ch_upper: 2,
                yu_lower: 2,
                yu_upper: 1
            }
        );
        let (s, _) = repair_letters("о^г, кл\u{43e}^~~евые");
        assert_eq!(s, "юг, ключевые");
        let (s, c) = repair_letters("1. e4 {plain}");
        assert_eq!((s.as_str(), c.total()), ("1. e4 {plain}", 0));
    }

    fn enc1251(s: &str) -> Vec<u8> {
        encoding_rs::WINDOWS_1251.encode(s).0.into_owned()
    }

    #[test]
    fn detection() {
        let bad = "{Клo^~~евые поля, ><ерные играo^т, ~~ерные} ".repeat(10);
        let d = detect_letter_substitution(&enc1251(&bad), TextEncoding::Windows1251);
        assert!(d.detected, "{d:?}");
        assert_eq!(d.genuine_letters, 0);
        // Genuine letters present: no repair.
        let mixed = format!("{bad} {}", "ключевые черные ".repeat(5));
        assert!(!detect_letter_substitution(&enc1251(&mixed), TextEncoding::Windows1251).detected);
        // UTF-8 or Western sources are never repaired.
        assert!(!detect_letter_substitution(bad.as_bytes(), TextEncoding::Utf8).detected);
        // Too few sequences.
        let few = "{Клo^~~евые}";
        assert!(!detect_letter_substitution(&enc1251(few), TextEncoding::Windows1251).detected);
        // Many unrelated tildes (ASCII art) disable the repair.
        let art = format!("{bad}{}", "~~~~~~~~ ^^^^ ".repeat(40));
        assert!(!detect_letter_substitution(&enc1251(&art), TextEncoding::Windows1251).detected);
    }
}
