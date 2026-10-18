//! Text decoding and guiding texts.
//!
//! ChessBase does not record the code page of its texts: a text is UTF-8 or a
//! single-byte code page. A database is decoded with one single-byte code page,
//! detected from all its texts at once, while each text that is valid UTF-8 (and
//! not plain ASCII) is taken as UTF-8.

use chess_core::{TextEncoding, detect_encoding};

use super::be;

/// The byte ChessBase uses in single-byte texts to place a diagram.
const DIAGRAM: u8 = 0x9e;
/// The same marker in UTF-8 texts (a private-use character).
const DIAGRAM_UTF8: char = '\u{e005}';
/// Where a text embeds an object (a diagram, a picture, a link to a game or chapter);
/// what the object is lies in formatting data whose layout is unknown.
pub const OBJECT: char = '\u{4}';

/// Piece figurines in single-byte texts: bytes 0xa2-0xa7 stand for K Q N B R and a pawn
/// when a square or a capture follows ("¤f3", "¥:g4").
const FIGURINES_SINGLE: [&str; 6] = ["K", "Q", "N", "B", "R", ""];
/// Piece figurines in UTF-8 texts: U+E024-U+E029 stand for K Q R B N and a pawn.
const FIGURINES_UTF8: [&str; 6] = ["K", "Q", "R", "B", "N", ""];

/// Adds a text to a code page detection sample. UTF-8 texts are decoded as such one
/// by one (see [`Decoder`]) and would only mislead the detection of the single-byte
/// code page, so they are left out, as is plain ASCII.
pub fn add_sample(buf: &mut Vec<u8>, text: &[u8]) {
    if !text.is_ascii() && std::str::from_utf8(text).is_err() {
        buf.extend_from_slice(text);
        buf.push(b' ');
    }
}

pub fn detect(sample: &[u8]) -> TextEncoding {
    match detect_encoding(sample).encoding {
        // All ASCII or UTF-8: single-byte texts (if any) are most likely Western.
        TextEncoding::Utf8 => TextEncoding::Windows1252,
        e => e,
    }
}

pub struct Decoder {
    single_byte: TextEncoding,
}

impl Decoder {
    pub fn new(single_byte: TextEncoding) -> Decoder {
        Decoder { single_byte }
    }

    pub fn single_byte(&self) -> TextEncoding {
        self.single_byte
    }

    /// A comment or a guiding text. Control characters are dropped, apart from line
    /// breaks, tabs and [`OBJECT`].
    pub fn decode(&self, bytes: &[u8]) -> String {
        let mut s = self.decode_raw(bytes);
        s.retain(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t' | OBJECT));
        s
    }

    fn decode_raw(&self, bytes: &[u8]) -> String {
        if !bytes.is_ascii()
            && let Ok(s) = std::str::from_utf8(bytes)
        {
            let mut out = String::with_capacity(s.len());
            for c in s.chars() {
                match c {
                    DIAGRAM_UTF8 => {}
                    '\u{e024}'..='\u{e029}' => {
                        out.push_str(FIGURINES_UTF8[(c as u32 - 0xe024) as usize]);
                    }
                    c => out.push(c),
                }
            }
            return out;
        }
        let mut cleaned = Vec::with_capacity(bytes.len());
        for (i, &b) in bytes.iter().enumerate() {
            let next = bytes.get(i + 1).copied().unwrap_or(0);
            match b {
                DIAGRAM => {}
                0xa2..=0xa7 if matches!(next, b'a'..=b'h' | b'x' | b':') => {
                    cleaned.extend_from_slice(FIGURINES_SINGLE[usize::from(b - 0xa2)].as_bytes());
                }
                b => cleaned.push(b),
            }
        }
        self.single_byte.decode(&cleaned).0.into_owned()
    }

    /// A name of a player, tournament or annotator. ChessBase stores names in a Western
    /// code page even in a database whose texts are Cyrillic, but a name may also be
    /// typed in Cyrillic: the reading that does not mix Latin and Cyrillic letters
    /// within a word wins ("Nürnberg", not "Nьrnberg").
    pub fn decode_name(&self, bytes: &[u8]) -> String {
        let own = self.decode(bytes).replace(OBJECT, " ");
        if bytes.is_ascii()
            || std::str::from_utf8(bytes).is_ok()
            || !matches!(
                self.single_byte,
                TextEncoding::Windows1251 | TextEncoding::Koi8R | TextEncoding::Ibm866
            )
        {
            return own;
        }
        let mut western = TextEncoding::Windows1252.decode(bytes).0.into_owned();
        western.retain(|c| !c.is_control());
        if mixed_words(&western) < mixed_words(&own) {
            western
        } else {
            own
        }
    }
}

/// Words that contain both Latin and Cyrillic letters.
fn mixed_words(s: &str) -> usize {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| {
            let latin = w.chars().any(|c| c.is_alphabetic() && (c as u32) < 0x250);
            let cyrillic = w.chars().any(|c| ('\u{400}'..='\u{4ff}').contains(&c));
            latin && cyrillic
        })
        .count()
}

pub struct Body {
    pub title: String,
    pub text: String,
}

/// Reads a guiding text record from `.cbg`: titles and contents in one or more
/// languages; the first of each is used.
pub fn guiding_text(cbg: &[u8], offset: u32, d: &Decoder) -> Result<Body, String> {
    let at = offset as usize;
    let size = be(cbg, at + 1, 3).ok_or("text outside the .cbg file")? as usize;
    let rec = cbg.get(at..at + size).ok_or("text outside the .cbg file")?;
    let mut r = Le { data: rec, pos: 4 };
    let bad = || "damaged guiding text".to_owned();
    let version = r.u16().ok_or_else(bad)?;
    let mut title = String::new();
    for i in 0..r.u16().ok_or_else(bad)? {
        let _lang = r.u16().ok_or_else(bad)?;
        let len = usize::from(r.u16().ok_or_else(bad)?);
        let t = r.take(len).ok_or_else(bad)?;
        if i == 0 {
            title = d.decode(t).trim().to_owned();
        }
    }
    r.take(1).ok_or_else(bad)?;
    let mut text = String::new();
    for i in 0..r.u16().ok_or_else(bad)? {
        let _lang = r.u16().ok_or_else(bad)?;
        let body = match version {
            1 => {
                let len = usize::from(r.u16().ok_or_else(bad)?);
                let t = r.take(len).ok_or_else(bad)?;
                let fmt = usize::from(r.u16().ok_or_else(bad)?);
                r.take(fmt).ok_or_else(bad)?;
                tidy(&objects_to_breaks(&d.decode(t)))
            }
            2 => {
                let len = r.u32().ok_or_else(bad)? as usize;
                let t = r.take(len).ok_or_else(bad)?;
                let fmt = r.u32().ok_or_else(bad)? as usize;
                r.take(fmt).ok_or_else(bad)?;
                tidy(&objects_to_breaks(&d.decode(t)))
            }
            3 => {
                let len = r.u32().ok_or_else(bad)? as usize;
                let t = r.take(len).ok_or_else(bad)?;
                r.take(4);
                html_to_text(&d.decode(t).replace(OBJECT, " "))
            }
            v => return Err(format!("guiding text format {v} is not supported")),
        };
        if i == 0 {
            text = body;
        }
    }
    Ok(Body { title, text })
}

struct Le<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Le<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.data.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Readable text from the HTML of a guiding text: block elements become line
/// breaks, scripts and styles are dropped, entities are decoded.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..lt]));
        let Some(gt) = rest[lt..].find('>') else {
            rest = "";
            break;
        };
        let tag = rest[lt + 1..lt + gt].trim().to_ascii_lowercase();
        rest = &rest[lt + gt + 1..];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if !tag.starts_with('/') && matches!(name.as_str(), "script" | "style" | "head" | "title") {
            let close = format!("</{name}");
            match rest.to_ascii_lowercase().find(&close) {
                Some(end) => {
                    rest = &rest[end..];
                    if let Some(gt) = rest.find('>') {
                        rest = &rest[gt + 1..];
                    }
                }
                None => rest = "",
            }
            continue;
        }
        if matches!(
            name.as_str(),
            "br" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "table"
        ) && !out.ends_with('\n')
        {
            out.push('\n');
        }
    }
    out.push_str(&decode_entities(rest));
    tidy(&out)
}

/// Plain guiding texts carry no line breaks: paragraphs start after an embedded
/// object (a diagram, a link). A run of objects becomes a paragraph break when a
/// capital letter or a digit follows, and a space otherwise ("games (• and •)").
pub fn objects_to_breaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != OBJECT {
            out.push(c);
            continue;
        }
        while chars
            .peek()
            .is_some_and(|c| *c == OBJECT || c.is_whitespace())
        {
            chars.next();
        }
        let next = chars.peek().copied();
        if next.is_none_or(|n| n.is_uppercase() || n.is_ascii_digit()) {
            out.push('\n');
        } else {
            out.push(' ');
        }
    }
    out
}

/// One paragraph per non-empty line, separated by a blank line; whitespace inside a
/// line collapsed.
pub fn tidy(text: &str) -> String {
    // Line breaks may be "\r\n", "\n" or a lone "\r" (older ChessBase texts).
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';').filter(|i| *i <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[1..semi];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|n| n.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_texts_win_over_the_code_page() {
        let d = Decoder::new(TextEncoding::Windows1251);
        assert_eq!(d.decode("Привет".as_bytes()), "Привет");
        assert_eq!(d.decode(&[0xcf, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2]), "Привет");
        assert_eq!(d.decode(b"plain"), "plain");
    }

    #[test]
    fn figurines_and_diagrams() {
        let d = Decoder::new(TextEncoding::Windows1252);
        assert_eq!(
            d.decode(b"3...\xa5g4 4 \xa4:e5 \xa6xh5 \xa7b2+ \x9e"),
            "3...Bg4 4 N:e5 Rxh5 b2+ "
        );
        assert_eq!(
            d.decode(b"price \xa3 5"),
            "price £ 5",
            "not before a square"
        );
        assert_eq!(
            d.decode("plan \u{e027}e3, \u{e025}d2 \u{e028}c6 \u{e005}".as_bytes()),
            "plan Be3, Qd2 Nc6 "
        );
    }

    #[test]
    fn names_pick_the_code_page_per_name() {
        let d = Decoder::new(TextEncoding::Windows1251);
        assert_eq!(d.decode_name(b"N\xfcrnberg"), "Nürnberg");
        assert_eq!(d.decode_name(b"\xc0\xeb\xe5\xf5\xe8\xed"), "Алехин");
        assert_eq!(
            d.decode_name(b"1.7.\xe0"),
            "1.7.а",
            "a tie keeps the database code page"
        );
        // Comments keep the database code page even when Latin squares follow Cyrillic.
        assert_eq!(d.decode(b"\xcaf3"), "Кf3");
    }

    #[test]
    fn embedded_objects_split_paragraphs() {
        let t =
            "Title! \u{4} Body text (\u{4} and \u{4}) here. \u{4}\u{4} \u{4}УПРАЖНЕНИЯ \u{4} \u{4}";
        assert_eq!(
            tidy(&objects_to_breaks(t)),
            "Title!\n\nBody text ( and ) here.\n\nУПРАЖНЕНИЯ"
        );
        let d = Decoder::new(TextEncoding::Windows1252);
        assert_eq!(d.decode(b"a\x01b\x81c\x04d"), "abc\u{4}d");
    }

    #[test]
    fn html_becomes_text() {
        let html = "<html><head><title>x</title><style>p{}</style></head><body>\
                    <p>First&nbsp;line &amp; more</p><p>Second<br>third &#1095;</p>\
                    <script>alert(1)</script></body></html>";
        assert_eq!(html_to_text(html), "First line & more\n\nSecond\n\nthird ч");
        assert_eq!(tidy("\t\n  A  b \n\n\n\tc\n "), "A b\n\nc");
        assert_eq!(tidy("One\r\rTwo\r\nThree"), "One\n\nTwo\n\nThree");
        assert_eq!(html_to_text("a & b; <b>c</b>"), "a & b; c");
    }
}
