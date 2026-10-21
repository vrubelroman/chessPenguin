//! Embedded comment commands (`[%csl …]`, `[%cal …]`, any `[%name …]`).
//!
//! Comments are stored verbatim in the game; this module only interprets them
//! for display. Unknown commands are returned as [`Annotation::Other`].

use shakmaty::Square;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ShapeColor {
    Red,
    Green,
    Blue,
    Yellow,
    Other(char),
}

impl ShapeColor {
    fn from_char(c: char) -> ShapeColor {
        match c {
            'R' => ShapeColor::Red,
            'G' => ShapeColor::Green,
            'B' => ShapeColor::Blue,
            'Y' => ShapeColor::Yellow,
            c => ShapeColor::Other(c),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Annotation {
    /// `[%csl Ra6,Gb7]` highlighted squares.
    Squares(Vec<(ShapeColor, Square)>),
    /// `[%cal Rc7b6]` arrows.
    Arrows(Vec<(ShapeColor, Square, Square)>),
    /// Any other command (or a malformed csl/cal), payload kept verbatim.
    Other { name: String, payload: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommentPart {
    Text(String),
    Annotation(Annotation),
}

/// Splits a comment into plain text and commands, in order. Text parts are
/// trimmed; empty text parts are omitted.
pub fn parse_comment(text: &str) -> Vec<CommentPart> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[%") {
        push_text(&mut parts, &rest[..start]);
        let body = &rest[start + 2..];
        // Find the closing ']' outside double quotes.
        let mut in_quotes = false;
        let mut end = None;
        for (i, c) in body.char_indices() {
            match c {
                '"' => in_quotes = !in_quotes,
                ']' if !in_quotes => {
                    end = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            push_text(&mut parts, &rest[start..]);
            return parts;
        };
        let inner = body[..end].trim();
        let (name, payload) = match inner.find(char::is_whitespace) {
            Some(i) => (&inner[..i], inner[i..].trim()),
            None => (inner, ""),
        };
        parts.push(CommentPart::Annotation(command(name, payload)));
        rest = &body[end + 1..];
    }
    push_text(&mut parts, rest);
    parts
}

fn push_text(parts: &mut Vec<CommentPart>, s: &str) {
    let t = s.trim();
    if !t.is_empty() {
        parts.push(CommentPart::Text(t.to_owned()));
    }
}

fn command(name: &str, payload: &str) -> Annotation {
    let other = || Annotation::Other {
        name: name.to_owned(),
        payload: payload.to_owned(),
    };
    let items = payload.split(',').map(str::trim).filter(|s| !s.is_empty());
    match name {
        "csl" => {
            let mut v = Vec::new();
            for it in items {
                let mut ch = it.chars();
                let (Some(c), sq) = (ch.next(), ch.as_str()) else {
                    return other();
                };
                match sq.parse::<Square>() {
                    Ok(sq) => v.push((ShapeColor::from_char(c), sq)),
                    Err(_) => return other(),
                }
            }
            Annotation::Squares(v)
        }
        "cal" => {
            let mut v = Vec::new();
            for it in items {
                let mut ch = it.chars();
                let Some(c) = ch.next() else { return other() };
                let rest = ch.as_str();
                let (Some(a), Some(b)) = (rest.get(..2), rest.get(2..)) else {
                    return other();
                };
                match (a.parse::<Square>(), b.parse::<Square>()) {
                    (Ok(a), Ok(b)) => v.push((ShapeColor::from_char(c), a, b)),
                    _ => return other(),
                }
            }
            Annotation::Arrows(v)
        }
        _ => other(),
    }
}

/// The text of a node's comments as a reader sees it: board commands (`[%cal]`,
/// `[%clk]`, …) left out, single line breaks reflowed (they are formatting), a blank
/// line kept as a paragraph break (book chapters imported from ChessBase).
pub fn comment_display_text(comments: &[String]) -> String {
    let mut out = String::new();
    for c in comments {
        for part in parse_comment(c) {
            if let CommentPart::Text(t) = part {
                let mut blank = false;
                for line in t.lines() {
                    if line.trim().is_empty() {
                        blank = true;
                        continue;
                    }
                    if blank && !out.is_empty() {
                        out.push_str("\n\n");
                    }
                    blank = false;
                    for word in line.split_whitespace() {
                        if !out.is_empty() && !out.ends_with('\n') {
                            out.push(' ');
                        }
                        out.push_str(word);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shapes_and_keeps_unknown() {
        let parts = parse_comment(
            "[%csl Rc6,Gd6] Ключевые поля [%cal Rc7b6] [%tqu \"Ход?\",\"\",Ka6,\"]\",1] конец",
        );
        assert_eq!(parts.len(), 5);
        assert_eq!(
            parts[0],
            CommentPart::Annotation(Annotation::Squares(vec![
                (ShapeColor::Red, Square::C6),
                (ShapeColor::Green, Square::D6)
            ]))
        );
        assert_eq!(parts[1], CommentPart::Text("Ключевые поля".into()));
        assert_eq!(
            parts[2],
            CommentPart::Annotation(Annotation::Arrows(vec![(
                ShapeColor::Red,
                Square::C7,
                Square::B6
            )]))
        );
        assert!(
            matches!(&parts[3], CommentPart::Annotation(Annotation::Other { name, .. }) if name == "tqu")
        );
        assert_eq!(parts[4], CommentPart::Text("конец".into()));
    }

    #[test]
    fn unterminated_command_is_text() {
        assert_eq!(
            parse_comment("a [%csl Rc6"),
            vec![
                CommentPart::Text("a".into()),
                CommentPart::Text("[%csl Rc6".into())
            ]
        );
    }
}
