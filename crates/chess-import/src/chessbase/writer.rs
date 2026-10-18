//! PGN text for decoded games and guiding texts.

use std::fmt::Write as _;

use super::annotations::{Annotations, Item};
use super::entities::Entities;
use super::headers::{GameHeader, TextHeader, date_tag, round_tag};
use super::moves::Tree;
use super::text::Decoder;

pub type Tags = Vec<(String, String)>;

pub fn tags(
    h: &GameHeader,
    players: &Entities,
    tournaments: &Entities,
    annotators: &Entities,
    d: &Decoder,
) -> Tags {
    let or_q = |s: Option<String>| s.unwrap_or_else(|| "?".into());
    let event_date = tournaments.date(h.tournament);
    let mut t: Tags = vec![
        ("Event".into(), or_q(tournaments.name(h.tournament, d))),
        ("Site".into(), or_q(tournaments.place(h.tournament, d))),
        ("Date".into(), date_tag(h.date)),
        ("Round".into(), round_tag(h.round, h.subround)),
        ("White".into(), or_q(players.name(h.white, d))),
        ("Black".into(), or_q(players.name(h.black, d))),
        ("Result".into(), h.result_tag().into()),
    ];
    if h.white_elo > 0 {
        t.push(("WhiteElo".into(), h.white_elo.to_string()));
    }
    if h.black_elo > 0 {
        t.push(("BlackElo".into(), h.black_elo.to_string()));
    }
    if let Some(eco) = h.eco_tag() {
        t.push(("ECO".into(), eco));
    }
    if let Some(a) = annotators.name(h.annotator, d) {
        t.push(("Annotator".into(), a));
    }
    if event_date != 0 {
        t.push(("EventDate".into(), date_tag(event_date)));
    }
    t
}

/// A guiding text is shown as a game without moves: the title in place of the
/// players and the text as the opening comment.
pub fn text_tags(
    h: &TextHeader,
    title: &str,
    tournaments: &Entities,
    annotators: &Entities,
    d: &Decoder,
) -> Tags {
    let mut t: Tags = vec![
        (
            "Event".into(),
            tournaments
                .name(h.tournament, d)
                .unwrap_or_else(|| "?".into()),
        ),
        ("Site".into(), "?".into()),
        ("Date".into(), "????.??.??".into()),
        ("Round".into(), round_tag(h.round, h.subround)),
        (
            "White".into(),
            if title.is_empty() {
                "?".into()
            } else {
                title.into()
            },
        ),
        ("Black".into(), String::new()),
        ("Result".into(), "*".into()),
    ];
    if let Some(a) = annotators.name(h.annotator, d) {
        t.push(("Annotator".into(), a));
    }
    t
}

pub fn text_game(out: &mut String, tags: &Tags, text: &str) {
    write_tags(out, tags, None);
    let mut w = Movetext::default();
    if !text.trim().is_empty() {
        w.comment(text);
    }
    w.token("*");
    out.push_str(&w.finish());
}

pub fn game(out: &mut String, tags: &Tags, tree: &Tree, notes: &Annotations, result: &str) {
    // Main-line length, for the library's move count.
    let mut plies = 0;
    let mut node = 0;
    while let Some(&next) = tree.nodes[node].children.first() {
        plies += 1;
        node = next;
    }
    let mut tags = tags.clone();
    if plies > 0 {
        tags.push(("PlyCount".into(), plies.to_string()));
    }
    write_tags(out, &tags, tree.start_fen.as_deref());
    // Annotation position of each node.
    let mut position = vec![None; tree.nodes.len()];
    for (i, n) in tree.order.iter().enumerate() {
        position[*n] = i32::try_from(i).ok();
    }
    let mut w = Movetext::default();
    if let Some(items) = notes.at.get(&-1) {
        // Before-move texts of the whole game read as an introduction too.
        let mut intro: Vec<&Item> = items.iter().collect();
        intro.sort_by_key(|i| !matches!(i, Item::Before(_)));
        w.after_comment(&intro);
    }
    let ctx = Ctx {
        tree,
        notes,
        position: &position,
    };
    ctx.line(&mut w, 0);
    w.token(result);
    out.push_str(&w.finish());
}

struct Ctx<'a> {
    tree: &'a Tree,
    notes: &'a Annotations,
    position: &'a [Option<i32>],
}

impl Ctx<'_> {
    /// Writes the continuation of `node`: its main move, the alternatives to that
    /// move as variations, then the rest of the main line.
    fn line(&self, w: &mut Movetext, mut node: usize) {
        loop {
            let children = &self.tree.nodes[node].children;
            let Some(&main) = children.first() else {
                return;
            };
            self.write_move(w, main);
            for &alt in &children[1..] {
                w.open();
                self.write_move(w, alt);
                self.line(w, alt);
                w.close();
            }
            node = main;
        }
    }

    fn write_move(&self, w: &mut Movetext, id: usize) {
        let n = &self.tree.nodes[id];
        let items: &[Item] = self.position[id]
            .and_then(|p| self.notes.at.get(&p))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for it in items {
            if let Item::Before(t) = it {
                w.comment(t);
            }
        }
        // The number and the move stay on one line.
        let number = if n.white {
            format!("{}. ", n.number)
        } else if w.need_number {
            format!("{}... ", n.number)
        } else {
            String::new()
        };
        w.need_number = false;
        w.token(&format!("{number}{}", n.san));
        for it in items {
            if let Item::Nags(nags) = it {
                for nag in nags {
                    w.token(&format!("${nag}"));
                }
            }
        }
        let after: Vec<&Item> = items
            .iter()
            .filter(|i| !matches!(i, Item::Before(_) | Item::Nags(_)))
            .collect();
        w.after_comment(&after);
    }
}

fn write_tags(out: &mut String, tags: &Tags, fen: Option<&str>) {
    for (k, v) in tags {
        let _ = writeln!(out, "[{k} \"{}\"]", escape_tag(v));
    }
    if let Some(fen) = fen {
        let _ = writeln!(out, "[SetUp \"1\"]");
        let _ = writeln!(out, "[FEN \"{}\"]", escape_tag(fen));
    }
    out.push('\n');
}

fn escape_tag(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\r', '\n'], " ")
}

/// Movetext builder with line wrapping at 80 columns.
#[derive(Default)]
struct Movetext {
    lines: Vec<String>,
    current: String,
    need_number: bool,
}

impl Movetext {
    fn token(&mut self, t: &str) {
        if !self.current.is_empty() && self.current.chars().count() + 1 + t.chars().count() > 80 {
            self.lines.push(std::mem::take(&mut self.current));
        }
        if !self.current.is_empty() && !self.current.ends_with('(') {
            self.current.push(' ');
        }
        self.current.push_str(t);
    }

    fn comment(&mut self, text: &str) {
        // '}' would end the comment early; ChessBase texts rarely contain it.
        let clean = text.replace('}', ")");
        self.token(&format!("{{{clean}}}"));
        self.need_number = true;
    }

    /// One comment with the graphics as `[%csl]`/`[%cal]`/`[%emt]` commands and the texts.
    fn after_comment(&mut self, items: &[&Item]) {
        let mut parts: Vec<String> = Vec::new();
        for it in items {
            match it {
                Item::Squares(list) if !list.is_empty() => parts.push(format!(
                    "[%csl {}]",
                    list.iter()
                        .map(|(c, s)| format!("{c}{}", square(*s)))
                        .collect::<Vec<_>>()
                        .join(",")
                )),
                Item::Arrows(list) if !list.is_empty() => parts.push(format!(
                    "[%cal {}]",
                    list.iter()
                        .map(|(c, a, b)| format!("{c}{}{}", square(*a), square(*b)))
                        .collect::<Vec<_>>()
                        .join(",")
                )),
                Item::TimeSpent(h, m, s) => parts.push(format!("[%emt {h}:{m:02}:{s:02}]")),
                Item::After(t) | Item::Before(t) => parts.push(t.clone()),
                _ => {}
            }
        }
        if !parts.is_empty() {
            self.comment(&parts.join(" "));
        }
    }

    fn open(&mut self) {
        self.token("(");
        self.need_number = true;
    }

    fn close(&mut self) {
        self.current.push(')');
        self.need_number = true;
    }

    fn finish(mut self) -> String {
        if !self.current.is_empty() {
            self.lines.push(self.current);
        }
        let mut s = self.lines.join("\n");
        s.push_str("\n\n");
        s
    }
}

/// Square name from ChessBase numbering (a1 = 0, a2 = 1, … h8 = 63).
fn square(idx: u8) -> String {
    let file = (b'a' + idx / 8) as char;
    let rank = (b'1' + idx % 8) as char;
    format!("{file}{rank}")
}

#[cfg(test)]
mod tests {
    use super::super::moves::Node;
    use super::*;

    fn node(san: &str, number: u32, white: bool, children: Vec<usize>) -> Node {
        Node {
            san: san.into(),
            number,
            white,
            children,
        }
    }

    #[test]
    fn variations_comments_and_graphics() {
        // 1. e4 c5 (1... c6 2. d4) 2. Nf3
        let tree = Tree {
            start_fen: None,
            nodes: vec![
                node("", 0, true, vec![1]),
                node("e4", 1, true, vec![2, 4]),
                node("c5", 1, false, vec![3]),
                node("Nf3", 2, true, vec![]),
                node("c6", 1, false, vec![5]),
                node("d4", 2, true, vec![]),
            ],
            order: vec![1, 2, 3, 4, 5],
        };
        let mut notes = Annotations::default();
        notes.at.insert(-1, vec![Item::After("Intro".into())]);
        notes.at.insert(
            0,
            vec![
                Item::Nags(vec![1]),
                Item::Arrows(vec![('G', 35, 36)]),
                Item::After("Best by test }".into()),
            ],
        );
        notes.at.insert(3, vec![Item::Before("Also".into())]);
        let mut out = String::new();
        game(
            &mut out,
            &vec![("Event".into(), "Test \"x\"".into())],
            &tree,
            &notes,
            "*",
        );
        assert_eq!(
            out,
            "[Event \"Test \\\"x\\\"\"]\n[PlyCount \"3\"]\n\n{Intro} 1. e4 $1 {[%cal Ge4e5] Best by test )} 1... c5 ({Also} 1... c6 2. d4)\n\
             2. Nf3 *\n\n"
        );
    }
}
