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
    /// The letter PGN writes: `R`, `G`, `B`, `Y`.
    pub fn letter(self) -> char {
        match self {
            ShapeColor::Red => 'R',
            ShapeColor::Green => 'G',
            ShapeColor::Blue => 'B',
            ShapeColor::Yellow => 'Y',
            ShapeColor::Other(c) => c,
        }
    }

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

impl Annotation {
    /// The command as PGN writes it: `[%cal Rc7b6]`.
    pub fn to_pgn(&self) -> String {
        match self {
            Annotation::Squares(list) => format!(
                "[%csl {}]",
                list.iter()
                    .map(|(c, sq)| format!("{}{sq}", c.letter()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Annotation::Arrows(list) => format!(
                "[%cal {}]",
                list.iter()
                    .map(|(c, a, b)| format!("{}{a}{b}", c.letter()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Annotation::Other { name, payload } if payload.is_empty() => format!("[%{name}]"),
            Annotation::Other { name, payload } => format!("[%{name} {payload}]"),
        }
    }
}

/// The commands of a node's comments, in order.
fn commands(comments: &[String]) -> Vec<Annotation> {
    comments
        .iter()
        .flat_map(|c| parse_comment(c))
        .filter_map(|p| match p {
            CommentPart::Annotation(a) => Some(a),
            CommentPart::Text(_) => None,
        })
        .collect()
}

/// One comment from `text` and `commands`: the text, then the commands; nothing when
/// both are empty (an empty `Vec`, so that no `{}` is written).
fn assemble(text: &str, commands: &[Annotation]) -> Vec<String> {
    let mut out = text.trim().to_owned();
    for c in commands {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&c.to_pgn());
    }
    if out.is_empty() {
        Vec::new()
    } else {
        vec![out]
    }
}

/// The comments of a node with their text replaced by what the user typed: the
/// commands (`[%cal]`, `[%clk]`, …) stay, in their order, after the text. A `}` cannot
/// be in a PGN comment and becomes `)`. Paragraphs (blank lines) are kept, a single
/// line break is a space.
pub fn with_text(comments: &[String], text: &str) -> Vec<String> {
    let mut clean = String::new();
    let mut blank = false;
    for line in text.replace('}', ")").lines() {
        let line = line.trim();
        if line.is_empty() {
            blank = !clean.is_empty();
            continue;
        }
        if blank {
            clean.push_str("\n\n");
        } else if !clean.is_empty() {
            clean.push(' ');
        }
        blank = false;
        clean.push_str(line);
    }
    assemble(&clean, &commands(comments))
}

/// Arrows `(colour, from, to)` and squares `(colour, square)` of a node.
pub type Shapes = (Vec<(ShapeColor, Square, Square)>, Vec<(ShapeColor, Square)>);

/// The arrows and the squares drawn on the board for a node (`[%cal]`, `[%csl]`).
pub fn shapes(comments: &[String]) -> Shapes {
    let mut arrows = Vec::new();
    let mut squares = Vec::new();
    for a in commands(comments) {
        match a {
            Annotation::Arrows(list) => arrows.extend(list),
            Annotation::Squares(list) => squares.extend(list),
            Annotation::Other { .. } => {}
        }
    }
    (arrows, squares)
}

/// The comments of a node with these arrows and squares instead of its own: the text
/// and the other commands stay.
pub fn with_shapes(
    comments: &[String],
    arrows: &[(ShapeColor, Square, Square)],
    squares: &[(ShapeColor, Square)],
) -> Vec<String> {
    let mut cmds: Vec<Annotation> = commands(comments)
        .into_iter()
        .filter(|a| matches!(a, Annotation::Other { .. }))
        .collect();
    if !squares.is_empty() {
        cmds.push(Annotation::Squares(squares.to_vec()));
    }
    if !arrows.is_empty() {
        cmds.push(Annotation::Arrows(arrows.to_vec()));
    }
    assemble(&comment_display_text(comments), &cmds)
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
    #[test]
    fn with_text_keeps_commands_and_paragraphs() {
        let c = vec!["Старый текст [%cal Gf3d5][%clk 0:03:12]".to_owned()];
        assert_eq!(
            with_text(&c, "  Новый }текст{\nещё\n\n\nабзац  "),
            vec!["Новый )текст{ ещё\n\nабзац [%cal Gf3d5] [%clk 0:03:12]".to_owned()]
        );
        // No text: the commands alone; nothing at all: an empty Vec.
        assert_eq!(
            with_text(&c, " "),
            vec!["[%cal Gf3d5] [%clk 0:03:12]".to_owned()]
        );
        assert!(with_text(&["plain".to_owned()], "").is_empty());
        assert_eq!(with_text(&[], "a"), vec!["a".to_owned()]);
    }

    #[test]
    fn with_shapes_replaces_board_commands_only() {
        let c = vec![
            "Текст [%cal Gf3d5] [%clk 0:03:12]".to_owned(),
            "[%csl Ra6]".to_owned(),
        ];
        let (arrows, squares) = shapes(&c);
        assert_eq!(arrows.len(), 1);
        assert_eq!(squares.len(), 1);
        let out = with_shapes(&c, &[(ShapeColor::Red, Square::E2, Square::E4)], &[]);
        assert_eq!(out, vec!["Текст [%clk 0:03:12] [%cal Re2e4]".to_owned()]);
        assert_eq!(
            with_shapes(&c, &[], &[]),
            vec!["Текст [%clk 0:03:12]".to_owned()]
        );
    }
}
