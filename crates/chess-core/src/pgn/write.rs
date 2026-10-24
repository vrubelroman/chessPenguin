//! PGN export.
//!
//! Normalization (documented, intentional): LF line endings, movetext wrapped
//! at ~80 columns, whitespace inside comments collapsed, SAN regenerated from
//! the move, NAGs written as `$n`. Tags keep their order and values (including
//! the original FEN text, e.g. fullmove 0).

use shakmaty::{Color, Position};

use crate::game::{Game, NodeId};

#[derive(Clone, Debug)]
pub struct WriteOptions {
    pub max_line: usize,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions { max_line: 80 }
    }
}

struct Out {
    buf: String,
    line_len: usize,
    max: usize,
    glue_next: bool,
}

impl Out {
    fn token(&mut self, t: &str) {
        let len = t.chars().count();
        if self.glue_next {
            self.glue_next = false;
        } else if self.line_len > 0 {
            if self.line_len + 1 + len > self.max {
                self.buf.push('\n');
                self.line_len = 0;
            } else {
                self.buf.push(' ');
                self.line_len += 1;
            }
        }
        self.buf.push_str(t);
        self.line_len += len;
    }

    /// Appended directly to the previous token (`)`).
    fn glued(&mut self, t: &str) {
        self.buf.push_str(t);
        self.line_len += t.chars().count();
    }

    fn open_paren(&mut self) {
        self.token("(");
        self.glue_next = true;
    }

    fn newline(&mut self) {
        if self.line_len > 0 {
            self.buf.push('\n');
            self.line_len = 0;
        }
    }

    fn words(&mut self, text: &str) {
        for w in text.split_whitespace() {
            self.token(w);
        }
    }

    fn comment(&mut self, text: &str) {
        if text.contains('}') {
            // Cannot be represented in braces: write as `;` line comments.
            for line in text.lines() {
                self.token(&format!(";{}", line.trim()));
                self.newline();
            }
            return;
        }
        // Lines are reflowed, paragraphs (blank lines) kept: a book's comment keeps its
        // shape (comment_display_text shows a blank line as a paragraph break).
        let mut paragraphs: Vec<Vec<&str>> = vec![Vec::new()];
        for line in text.lines() {
            if line.trim().is_empty() {
                if paragraphs.last().is_some_and(|p| !p.is_empty()) {
                    paragraphs.push(Vec::new());
                }
            } else {
                paragraphs
                    .last_mut()
                    .expect("one paragraph")
                    .extend(line.split_whitespace());
            }
        }
        paragraphs.retain(|p| !p.is_empty());
        let total: usize = paragraphs.iter().map(Vec::len).sum();
        if total == 0 {
            self.token("{}");
            return;
        }
        let mut n = 0;
        for (k, words) in paragraphs.iter().enumerate() {
            if k > 0 {
                // A blank line inside the braces.
                self.newline();
                self.buf.push('\n');
            }
            for w in words {
                n += 1;
                let open = if n == 1 { "{" } else { "" };
                let close = if n == total { "}" } else { "" };
                self.token(&format!("{open}{w}{close}"));
            }
        }
    }
}

fn escape_tag(v: &str) -> String {
    let mut s = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => s.push_str("\\\\"),
            '"' => s.push_str("\\\""),
            '\n' | '\r' => s.push(' '),
            c => s.push(c),
        }
    }
    s
}

/// Serializes a game to PGN (UTF-8, LF).
pub fn write_game(game: &Game, options: &WriteOptions) -> String {
    let mut out = Out {
        buf: String::new(),
        line_len: 0,
        max: options.max_line.max(20),
        glue_next: false,
    };
    for (k, v) in game.tags() {
        out.buf.push_str(&format!("[{k} \"{}\"]\n", escape_tag(v)));
    }
    if game.tag("FEN").is_none()
        && let Some(fen) = game.start().original_fen()
    {
        if game.tag("SetUp").is_none() {
            out.buf.push_str("[SetUp \"1\"]\n");
        }
        out.buf
            .push_str(&format!("[FEN \"{}\"]\n", escape_tag(fen)));
    }
    out.buf.push('\n');

    let root = game.root();
    if let Some(r) = game.node(root) {
        for c in r.comments_before() {
            out.comment(c);
        }
        for c in r.comments_after() {
            out.comment(c);
        }
        for n in r.nags() {
            out.token(&format!("${}", n.0));
        }
        for u in r.unparsed() {
            out.words(u);
        }
    }
    write_line(game, root, true, &mut out);
    out.token(game.result().as_str());
    out.buf.push('\n');
    out.buf
}

/// Writes one move: comments before, number, SAN, NAGs, comments after.
/// Returns whether the next black move needs an explicit number.
fn write_move(game: &Game, id: NodeId, force_number: bool, out: &mut Out) -> bool {
    let Some(node) = game.node(id) else {
        return true;
    };
    let mut force = force_number;
    for c in node.comments_before() {
        out.comment(c);
        force = true;
    }
    let Some(mv) = node.game_move() else {
        return true;
    };
    if let Some(before) = game.position_before(id) {
        let n = before.fullmoves().get();
        match before.turn() {
            Color::White => out.token(&format!("{n}.")),
            Color::Black if force => out.token(&format!("{n}...")),
            Color::Black => {}
        }
    }
    out.token(&mv.san);
    for nag in node.nags() {
        out.token(&format!("${}", nag.0));
    }
    let mut after = false;
    for c in node.comments_after() {
        out.comment(c);
        after = true;
    }
    after
}

/// Writes the continuation of `parent` (main child, its alternatives, and so on).
fn write_line(game: &Game, parent: NodeId, force_first: bool, out: &mut Out) {
    let mut cur = parent;
    let mut force = force_first;
    loop {
        let children = game.children(cur);
        let Some(&main) = children.first() else {
            break;
        };
        force = write_move(game, main, force, out);
        for &alt in &children[1..] {
            out.open_paren();
            let f = write_move(game, alt, true, out);
            write_unparsed(game, alt, out);
            write_line(game, alt, f, out);
            out.glued(")");
            force = true;
        }
        if write_unparsed(game, main, out) {
            force = true;
        }
        cur = main;
    }
}

fn write_unparsed(game: &Game, id: NodeId, out: &mut Out) -> bool {
    let Some(node) = game.node(id) else {
        return false;
    };
    for u in node.unparsed() {
        out.words(u);
    }
    !node.unparsed().is_empty()
}

impl Game {
    /// PGN text with default options.
    pub fn to_pgn(&self) -> String {
        write_game(self, &WriteOptions::default())
    }
}
