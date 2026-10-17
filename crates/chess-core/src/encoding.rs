//! Text encoding detection and decoding for PGN sources.
//!
//! The scanner works on raw bytes: all supported encodings are ASCII-compatible,
//! so PGN structure (`[`, `]`, `"`, `{`, `}`, `(`, `)`, newlines) is located in the
//! original bytes and byte offsets always refer to the original file. Only text
//! payloads (tag values, comments) are decoded.

use std::borrow::Cow;

/// Encodings supported for PGN sources.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TextEncoding {
    Utf8,
    /// Cyrillic Windows code page (observed in the Dvoretsky textbook).
    Windows1251,
    Koi8R,
    /// DOS Cyrillic.
    Ibm866,
    /// Western Windows code page. ISO-8859-1 text decodes identically for all
    /// printable characters, so the two cannot be distinguished.
    Windows1252,
}

impl TextEncoding {
    pub const ALL: [TextEncoding; 5] = [
        TextEncoding::Utf8,
        TextEncoding::Windows1251,
        TextEncoding::Koi8R,
        TextEncoding::Ibm866,
        TextEncoding::Windows1252,
    ];

    fn encoding(self) -> &'static encoding_rs::Encoding {
        match self {
            TextEncoding::Utf8 => encoding_rs::UTF_8,
            TextEncoding::Windows1251 => encoding_rs::WINDOWS_1251,
            TextEncoding::Koi8R => encoding_rs::KOI8_R,
            TextEncoding::Ibm866 => encoding_rs::IBM866,
            TextEncoding::Windows1252 => encoding_rs::WINDOWS_1252,
        }
    }

    /// Stable label for settings/UI.
    pub fn label(self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "UTF-8",
            TextEncoding::Windows1251 => "windows-1251",
            TextEncoding::Koi8R => "KOI8-R",
            TextEncoding::Ibm866 => "IBM866",
            TextEncoding::Windows1252 => "windows-1252",
        }
    }

    pub fn from_label(label: &str) -> Option<TextEncoding> {
        TextEncoding::ALL
            .into_iter()
            .find(|e| e.label().eq_ignore_ascii_case(label))
    }

    /// Decodes bytes; returns the text and whether replacement characters were
    /// inserted (only possible for UTF-8). A BOM is not stripped here.
    pub fn decode(self, bytes: &[u8]) -> (Cow<'_, str>, bool) {
        self.encoding().decode_without_bom_handling(bytes)
    }
}

/// How sure the detector is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Confidence {
    /// Valid UTF-8 (or pure ASCII), or a UTF-8 BOM.
    Certain,
    /// Heuristic choice among legacy code pages; the UI should allow override.
    Likely,
}

/// Result of [`detect_encoding`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodingGuess {
    pub encoding: TextEncoding,
    pub confidence: Confidence,
    /// File starts with a UTF-8 BOM (3 bytes, skipped by the scanner).
    pub has_bom: bool,
    /// File contains only ASCII bytes.
    pub ascii_only: bool,
    /// Number of bytes >= 0x80 in the sample.
    pub non_ascii_bytes: usize,
}

const SAMPLE_LIMIT: usize = 512 * 1024;

/// Detects the most plausible encoding.
///
/// Policy: BOM → UTF-8; strict UTF-8 → UTF-8; otherwise a legacy code page is
/// chosen heuristically: isolated high bytes between ASCII letters indicate a
/// Western code page (windows-1252), runs of high bytes indicate Cyrillic, and
/// among Cyrillic code pages the one yielding mostly lowercase Cyrillic letters
/// wins (wrong Cyrillic decodings produce mostly uppercase or box drawing).
pub fn detect_encoding(bytes: &[u8]) -> EncodingGuess {
    let has_bom = bytes.starts_with(b"\xEF\xBB\xBF");
    let non_ascii_bytes = bytes.iter().filter(|b| **b >= 0x80).count();
    if has_bom {
        return EncodingGuess {
            encoding: TextEncoding::Utf8,
            confidence: Confidence::Certain,
            has_bom,
            ascii_only: false,
            non_ascii_bytes,
        };
    }
    if std::str::from_utf8(bytes).is_ok() {
        return EncodingGuess {
            encoding: TextEncoding::Utf8,
            confidence: Confidence::Certain,
            has_bom,
            ascii_only: non_ascii_bytes == 0,
            non_ascii_bytes,
        };
    }

    let sample = &bytes[..bytes.len().min(SAMPLE_LIMIT)];
    let mut high = 0usize;
    let mut high_adjacent = 0usize;
    for (i, b) in sample.iter().enumerate() {
        if *b >= 0x80 {
            high += 1;
            let prev = i.checked_sub(1).and_then(|p| sample.get(p));
            let next = sample.get(i + 1);
            if prev.is_some_and(|p| *p >= 0x80) || next.is_some_and(|n| *n >= 0x80) {
                high_adjacent += 1;
            }
        }
    }

    let encoding = if high_adjacent * 2 < high {
        TextEncoding::Windows1252
    } else {
        [
            TextEncoding::Windows1251,
            TextEncoding::Koi8R,
            TextEncoding::Ibm866,
        ]
        .into_iter()
        .max_by_key(|enc| cyrillic_score(&enc.decode(sample).0))
        .unwrap_or(TextEncoding::Windows1251)
    };

    EncodingGuess {
        encoding,
        confidence: Confidence::Likely,
        has_bom,
        ascii_only: false,
        non_ascii_bytes,
    }
}

fn cyrillic_score(text: &str) -> i64 {
    let mut score = 0i64;
    for ch in text.chars() {
        match ch {
            'а'..='я' | 'ё' => score += 2,
            'А'..='Я' | 'Ё' => score -= 1,
            c if !c.is_ascii() => score -= 3,
            _ => {}
        }
    }
    score
}

/// Decodes a payload (tag value or comment): replaces CRLF/CR by LF.
pub(crate) fn decode_text(encoding: TextEncoding, bytes: &[u8]) -> (String, bool) {
    let (text, had_errors) = encoding.decode(bytes);
    let text = if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.into_owned()
    };
    (text, had_errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_utf8_and_bom() {
        let g = detect_encoding("[White \"Иванов\"]".as_bytes());
        assert_eq!(g.encoding, TextEncoding::Utf8);
        assert_eq!(g.confidence, Confidence::Certain);
        let g = detect_encoding(b"\xEF\xBB\xBF[White \"x\"]");
        assert!(g.has_bom);
        assert_eq!(g.encoding, TextEncoding::Utf8);
        assert!(detect_encoding(b"1. e4 e5").ascii_only);
    }

    #[test]
    fn detects_cyrillic_code_pages() {
        let text = "{При пешке на пятой горизонтали ключевыми являются поля} 1. e4 *";
        for enc in [
            TextEncoding::Windows1251,
            TextEncoding::Koi8R,
            TextEncoding::Ibm866,
        ] {
            let (bytes, _, _) = enc.encoding().encode(text);
            assert_eq!(detect_encoding(&bytes).encoding, enc, "{enc:?}");
        }
    }

    #[test]
    fn detects_western() {
        let bytes = b"[White \"H\xFCbner, R.\"]\n[Site \"Troms\xF8\"]\n1. e4 *";
        assert_eq!(detect_encoding(bytes).encoding, TextEncoding::Windows1252);
    }

    #[test]
    fn decode_text_normalizes_newlines() {
        let (t, err) = decode_text(TextEncoding::Utf8, b"a\r\nb\rc");
        assert_eq!(t, "a\nb\nc");
        assert!(!err);
    }
}
