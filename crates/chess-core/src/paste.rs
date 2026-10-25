//! Pasting moves or a PGN game from the clipboard into a game as a variation
//! (docs/research/paste-variation.md §4, docs/design/paste-variation.md).
//!
//! [`plan`] reads the text and finds where it fits without touching the game;
//! [`Game::apply_paste`] then grafts the new moves — with their comments, NAGs and
//! nested variations — as one undo step.
//!
//! What the clipboard may hold:
//! - a PGN game (tags, or numbers from move 1): it hangs where its moves part from
//!   this game, along any branch; with no common start, where one of its positions
//!   occurs here (another move order); with neither it does not fit;
//! - a fragment (`Nf3 d5`, `12...Rd8`, UCI `e2e4 e7e5`): after the selected move,
//!   instead of it, or at the position of its move number on the current line.
//!
//! Nothing is inserted when any move of the line is impossible.

use shakmaty::fen::Fen;
use shakmaty::san::SanPlus;
use shakmaty::uci::UciMove;
use shakmaty::{Chess, Color, EnPassantMode, Position};

use crate::edit::{Command, EditError};
use crate::encoding::TextEncoding;
use crate::game::{Game, GameMove, MoveKind, NodeId, play_null};
use crate::pgn::{parse_game_range, parse_str, scan_games};

/// Where and what to insert: the subtree of `source` from `from` hangs from `at`.
#[derive(Clone, Debug)]
pub struct PastePlan {
    /// The node of this game the new moves follow.
    pub at: NodeId,
    source: Game,
    from: NodeId,
    /// The variations of the pasted game this one lacks, along the way: (where here,
    /// the pasted move) — inserted too.
    extra: Vec<(NodeId, NodeId)>,
    /// `at` has no continuation: the moves continue the line instead of branching.
    pub continuation: bool,
    /// The moves of the new line (its main line): number, side, SAN.
    pub line: Vec<(u32, Color, String)>,
    /// The game reached this position by another move order: those moves
    /// (`1.Nf3 c5 2.e4`), noted before the first move of the line.
    pub transposition: Option<String>,
    /// «White – Black» of the pasted game, noted after the last move of the line.
    pub source_ref: Option<String>,
    /// Games in the clipboard (the first one is pasted).
    pub games: usize,
}

/// Why nothing is pasted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasteError {
    /// The clipboard is empty or holds no moves.
    NoMoves,
    /// A fragment fits neither after the selected move nor instead of it (nor at
    /// its move number).
    DoesNotFit,
    /// A move in the middle of the line is impossible.
    Illegal {
        number: u32,
        side: Color,
        san: String,
    },
    /// The game shares no position with this one.
    Unrelated,
    /// All the moves are here already: the end of that line; `comments` — the
    /// pasted moves had comments or symbols (not carried over).
    AlreadyThere { at: NodeId, comments: bool },
}

/// Finds where `text` fits in `target` (the user stands on `cursor`).
pub fn plan(target: &Game, cursor: NodeId, text: &str) -> Result<PastePlan, PasteError> {
    // «12…Rd8» as the program itself writes black's moves.
    let text = text.trim().replace('…', "...");
    let text = text.as_str();
    if text.is_empty() {
        return Err(PasteError::NoMoves);
    }
    if text.starts_with('[') {
        // The games are only counted; the first one is read.
        let never = std::sync::atomic::AtomicBool::new(false);
        let entries = scan_games(text.as_bytes(), TextEncoding::Utf8, &mut |_| {}, &never)
            .unwrap_or_default();
        let source = match entries.first() {
            Some(e) => parse_game_range(
                text.as_bytes(),
                e.byte_range.clone(),
                e.first_line,
                TextEncoding::Utf8,
            ),
            None => parse_str(text),
        };
        return whole_game(target, source, entries.len().max(1));
    }
    let uci = uci_moves(text);
    let number = leading_number(text);
    // «1.e4 …» from the standard start: a game, with all its rules (a «1.d4» into a
    // Sicilian is refused, not hung at the start).
    if uci.is_none() && number == Some((1, Color::White)) && target.start().is_standard() {
        return whole_game(target, parse_str(text), 1);
    }
    // A fragment: after the selected move, instead of it, at its move number.
    let mut candidates = vec![cursor];
    if let Some(parent) = target.parent(cursor) {
        candidates.push(parent);
    }
    if let Some(n) = number
        && let Some(at) = by_number(target, cursor, n)
        && !candidates.contains(&at)
    {
        candidates.push(at);
    }
    for at in candidates {
        let Some(pos) = target.position(at) else {
            continue;
        };
        if number.is_some_and(|(n, side)| (pos.fullmoves().get(), pos.turn()) != (n, side)) {
            continue;
        }
        let movetext = match &uci {
            Some(moves) => match uci_to_san(pos, moves) {
                Ok(san) => san,
                Err(0) => continue,
                Err(k) => return Err(illegal_at(pos, moves, k)),
            },
            None => text.to_owned(),
        };
        let fen = target.fen(at).unwrap_or_default();
        let source = parse_str(&format!("[SetUp \"1\"]\n[FEN \"{fen}\"]\n\n{movetext}\n")).game;
        if source.main_child(NodeId::ROOT).is_none() {
            continue; // its first move is impossible here
        }
        if let Some(e) = illegal_in_main_line(&source) {
            return Err(e);
        }
        return from_node(target, at, &source, NodeId::ROOT, None, 1);
    }
    // Numbers or not, plain SAN may still be a game from the start.
    if uci.is_none() && number.is_none() {
        return match whole_game(target, parse_str(text), 1) {
            Err(PasteError::Unrelated | PasteError::NoMoves) => Err(PasteError::DoesNotFit),
            other => other,
        };
    }
    Err(PasteError::DoesNotFit)
}

/// A whole game: where its moves part from this game; else where one of its
/// positions occurs here; else it does not fit.
fn whole_game(
    target: &Game,
    parsed: crate::pgn::ParsedGame,
    games: usize,
) -> Result<PastePlan, PasteError> {
    let source = parsed.game;
    if source.main_child(NodeId::ROOT).is_none() {
        return Err(PasteError::NoMoves);
    }
    if let Some(e) = illegal_in_main_line(&source) {
        return Err(e);
    }
    let same_start = key(source.start().position()) == key(target.start().position());
    // An empty game takes it all.
    if same_start && target.main_child(NodeId::ROOT).is_none() {
        return from_node(target, NodeId::ROOT, &source, NodeId::ROOT, None, games);
    }
    // The moves in common, along any branch.
    if same_start && walk(target, NodeId::ROOT, &source, NodeId::ROOT).matched > 0 {
        return from_node(target, NodeId::ROOT, &source, NodeId::ROOT, None, games);
    }
    // Another move order: the first of its positions that occurs here.
    let mut path = vec![NodeId::ROOT];
    path.extend(source.mainline());
    let targets: Vec<NodeId> = std::iter::once(NodeId::ROOT)
        .chain(target.mainline())
        .chain(target.attached_nodes())
        .collect();
    for (k, &s) in path.iter().enumerate() {
        if k == 0 && same_start {
            continue;
        }
        let Some(sp) = source.position(s) else {
            continue;
        };
        let want = key(sp);
        let Some(&t) = targets
            .iter()
            .find(|&&t| target.position(t).is_some_and(|tp| key(tp) == want))
        else {
            continue;
        };
        let note = (k > 0).then(|| line_text(&source, &path[1..=k]));
        return from_node(target, t, &source, s, note, games);
    }
    Err(PasteError::Unrelated)
}

/// The plan for the moves of `source` after `s`, matched along `target` from `t`.
fn from_node(
    target: &Game,
    t: NodeId,
    source: &Game,
    s: NodeId,
    transposition: Option<String>,
    games: usize,
) -> Result<PastePlan, PasteError> {
    let Walk {
        last,
        next,
        mut extra,
        ..
    } = walk(target, t, source, s);
    // The main line is all here: its variations this game lacks may still be new.
    let (at, from) = match next {
        Some(from) => (last, from),
        None if !extra.is_empty() => extra.remove(0),
        None => {
            let comments = commented(source, s);
            return Err(PasteError::AlreadyThere { at: last, comments });
        }
    };
    let mut line = Vec::new();
    let mut cur = Some(from);
    while let Some(c) = cur {
        if let (Some(before), Some(mv)) = (source.position_before(c), source.game_move(c)) {
            line.push((before.fullmoves().get(), before.turn(), mv.san.clone()));
        }
        cur = source.main_child(c);
    }
    let name = |tag: &str| {
        source
            .tag(tag)
            .map(str::trim)
            .filter(|v| !v.is_empty() && *v != "?")
    };
    let source_ref = match (name("White"), name("Black")) {
        (None, None) => None,
        (w, b) => Some(format!("{} – {}", w.unwrap_or("?"), b.unwrap_or("?"))),
    };
    Ok(PastePlan {
        at,
        source: source.clone(),
        from,
        extra,
        continuation: target.children(at).is_empty(),
        line,
        transposition,
        source_ref,
        games,
    })
}

/// What [`walk`] found.
struct Walk {
    /// The last node in common.
    last: NodeId,
    /// The first move of the pasted main line that is not here.
    next: Option<NodeId>,
    /// How many moves matched.
    matched: usize,
    /// The pasted game's alternatives along the way that are not here: (where here,
    /// the pasted move).
    extra: Vec<(NodeId, NodeId)>,
}

/// Follows the main line of `source` from `s` through `target` from `t` while the
/// moves are there (by any branch).
fn walk(target: &Game, mut t: NodeId, source: &Game, mut s: NodeId) -> Walk {
    let child = |t: NodeId, kind: MoveKind| {
        target
            .children(t)
            .iter()
            .copied()
            .find(|&c| target.game_move(c).is_some_and(|m| m.kind == kind))
    };
    let mut matched = 0;
    let mut extra = Vec::new();
    while let Some(next) = source.main_child(s) {
        // The pasted game's variations here: kept unless this game has the move.
        for &alt in &source.children(s)[1..] {
            if let Some(kind) = source.game_move(alt).map(|m| m.kind)
                && child(t, kind).is_none()
            {
                extra.push((t, alt));
            }
        }
        let Some(kind) = source.game_move(next).map(|m| m.kind) else {
            break;
        };
        match child(t, kind) {
            Some(c) => {
                t = c;
                s = next;
                matched += 1;
            }
            None => {
                return Walk {
                    last: t,
                    next: Some(next),
                    matched,
                    extra,
                };
            }
        }
    }
    Walk {
        last: t,
        next: None,
        matched,
        extra,
    }
}

/// The moves walked from `s` on had comments or symbols (lost when all are here).
fn commented(source: &Game, s: NodeId) -> bool {
    let mut cur = source.main_child(s);
    while let Some(c) = cur {
        if let Some(n) = source.node(c)
            && (!n.nags().is_empty()
                || !n.comments_after().is_empty()
                || !n.comments_before().is_empty())
        {
            return true;
        }
        cur = source.main_child(c);
    }
    false
}

/// An impossible move on the main line of a parsed game (its variations may keep
/// raw text: it is carried over as it is).
fn illegal_in_main_line(source: &Game) -> Option<PasteError> {
    let mut main = vec![NodeId::ROOT];
    main.extend(source.mainline());
    let last = *main.last()?;
    let node = source.node(last)?;
    let raw = node.unparsed().first()?;
    // The first move word, without a number glued to it («3.Qh8»).
    let san = raw
        .split_whitespace()
        .find(|w| !is_number(w))?
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '…')
        .to_owned();
    let pos = source.position(last)?;
    Some(PasteError::Illegal {
        number: pos.fullmoves().get(),
        side: pos.turn(),
        san,
    })
}

/// The position key: pieces, side to move, castling, en passant (no counters).
fn key(pos: &Chess) -> String {
    Fen::from_position(pos, EnPassantMode::Legal)
        .to_string()
        .split(' ')
        .take(4)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `1.Nf3 c5 2.e4` for the given nodes of `game`.
fn line_text(game: &Game, nodes: &[NodeId]) -> String {
    let mut out = String::new();
    for (i, &n) in nodes.iter().enumerate() {
        let (Some(before), Some(mv)) = (game.position_before(n), game.game_move(n)) else {
            continue;
        };
        let num = before.fullmoves().get();
        if !out.is_empty() {
            out.push(' ');
        }
        match before.turn() {
            Color::White => out.push_str(&format!("{num}.{}", mv.san)),
            Color::Black if i == 0 => out.push_str(&format!("{num}...{}", mv.san)),
            Color::Black => out.push_str(&mv.san),
        }
    }
    out
}

/// The number of a move (`12.`, `12...`, `12…`), as a word of the text.
fn is_number(w: &str) -> bool {
    let digits = w.trim_end_matches(['.', '…']);
    !digits.is_empty() && digits.len() < w.len() && digits.chars().all(|c| c.is_ascii_digit())
}

/// `12.` → (12, White), `12...` / `12…` → (12, Black), when the text starts so.
fn leading_number(text: &str) -> Option<(u32, Color)> {
    let first = text.split_whitespace().next()?;
    let digits: String = first.chars().take_while(char::is_ascii_digit).collect();
    let rest = &first[digits.len()..];
    let n: u32 = digits.parse().ok()?;
    if rest.starts_with("...") || rest.starts_with('…') {
        Some((n, Color::Black))
    } else if rest.starts_with('.') {
        Some((n, Color::White))
    } else {
        None
    }
}

/// The words of a UCI line (`e2e4 e7e5 g1f3`, numbers allowed), if the text is one.
fn uci_moves(text: &str) -> Option<Vec<String>> {
    let words: Vec<&str> = text
        .split_whitespace()
        .filter(|w| !is_number(w) && !matches!(*w, "1-0" | "0-1" | "1/2-1/2" | "*"))
        .collect();
    let is_uci = |w: &str| {
        let b = w.as_bytes();
        (b.len() == 4 || b.len() == 5)
            && (b'a'..=b'h').contains(&b[0])
            && (b'1'..=b'8').contains(&b[1])
            && (b'a'..=b'h').contains(&b[2])
            && (b'1'..=b'8').contains(&b[3])
            && (b.len() == 4 || b"qrbn".contains(&b[4]))
    };
    (!words.is_empty() && words.iter().all(|w| is_uci(w)))
        .then(|| words.iter().map(|w| (*w).to_owned()).collect())
}

/// UCI moves played from `pos` as SAN; `Err(k)`: the `k`-th move is impossible.
fn uci_to_san(pos: &Chess, moves: &[String]) -> Result<String, usize> {
    let mut pos = pos.clone();
    let mut out = Vec::new();
    for (k, w) in moves.iter().enumerate() {
        let m = w
            .parse::<UciMove>()
            .ok()
            .and_then(|u| u.to_move(&pos).ok())
            .ok_or(k)?;
        out.push(SanPlus::from_move_and_play_unchecked(&mut pos, m).to_string());
    }
    Ok(out.join(" "))
}

/// The error for the `k`-th UCI move from `pos`.
fn illegal_at(pos: &Chess, moves: &[String], k: usize) -> PasteError {
    let mut pos = pos.clone();
    for w in &moves[..k] {
        if let Some(m) = w.parse::<UciMove>().ok().and_then(|u| u.to_move(&pos).ok()) {
            pos.play_unchecked(m);
        }
    }
    PasteError::Illegal {
        number: pos.fullmoves().get(),
        side: pos.turn(),
        san: moves[k].clone(),
    }
}

/// The node of the current line (the path to `cursor`, then on by the main line)
/// where the move of number `n` is to be made.
fn by_number(target: &Game, cursor: NodeId, (n, side): (u32, Color)) -> Option<NodeId> {
    let mut line = target.path(cursor);
    let mut cur = cursor;
    while let Some(next) = target.main_child(cur) {
        line.push(next);
        cur = next;
    }
    line.into_iter().find(|&id| {
        target
            .position(id)
            .is_some_and(|p| (p.fullmoves().get(), p.turn()) == (n, side))
    })
}

impl Game {
    /// Inserts what [`plan`] found: the new moves with their comments, symbols and
    /// variations, the move-order note and the source, as one undo step. Returns the
    /// first inserted node.
    pub fn apply_paste(&mut self, plan: &PastePlan) -> Result<NodeId, EditError> {
        let pos = self
            .position(plan.at)
            .ok_or(EditError::UnknownNode)?
            .clone();
        // Every move is checked before the game changes.
        check(&plan.source, plan.from, &pos)?;
        for &(at, from) in &plan.extra {
            let pos = self.position(at).ok_or(EditError::UnknownNode)?.clone();
            check(&plan.source, from, &pos)?;
        }
        self.edit_group(|g| g.paste_grafts(plan))
    }

    /// The grafts of [`Game::apply_paste`], checked already.
    fn paste_grafts(&mut self, plan: &PastePlan) -> Result<NodeId, EditError> {
        let first = self.graft(plan.at, &plan.source, plan.from)?;
        if let Some(note) = &plan.transposition
            && let Some(n) = self.node_mut(first)
        {
            n.comments_before.insert(0, note.clone());
        }
        if let Some(r) = &plan.source_ref {
            let end = self.line_end(first);
            if let Some(n) = self.node_mut(end) {
                n.comments_after.push(r.clone());
            }
        }
        let index = self.child_index(first).unwrap_or(0);
        self.record(Command::Attach {
            parent: plan.at,
            child: first,
            index,
        });
        for &(at, from) in &plan.extra {
            let id = self.graft(at, &plan.source, from)?;
            let index = self.child_index(id).unwrap_or(0);
            self.record(Command::Attach {
                parent: at,
                child: id,
                index,
            });
        }
        Ok(first)
    }

    /// Copies the subtree of `source` from `s` under `parent`, positions replayed
    /// here (the counters may differ from the source's).
    fn graft(&mut self, parent: NodeId, source: &Game, s: NodeId) -> Result<NodeId, EditError> {
        let pos = self.position(parent).ok_or(EditError::UnknownNode)?.clone();
        let node = source.node(s).ok_or(EditError::UnknownNode)?;
        let mv = node.game_move().ok_or(EditError::UnknownNode)?;
        let (gm, after) = play(&pos, mv)?;
        let id = self.push_child(parent, gm, after);
        if let Some(n) = self.node_mut(id) {
            n.nags = node.nags().to_vec();
            n.comments_before = node.comments_before().to_vec();
            n.comments_after = node.comments_after().to_vec();
            n.unparsed = node.unparsed().to_vec();
        }
        for &c in source.children(s) {
            self.graft(id, source, c)?;
        }
        Ok(id)
    }
}

/// The move `mv` from `pos`: the node's move and the position after it.
fn play(pos: &Chess, mv: &GameMove) -> Result<(GameMove, Chess), EditError> {
    match mv.kind {
        MoveKind::Normal(m) => {
            if !pos.is_legal(m) {
                return Err(EditError::IllegalMove(mv.san.clone()));
            }
            let mut after = pos.clone();
            let san = SanPlus::from_move_and_play_unchecked(&mut after, m).to_string();
            Ok((GameMove { kind: mv.kind, san }, after))
        }
        MoveKind::Null => {
            let after = play_null(pos).map_err(|_| EditError::IllegalMove("--".into()))?;
            Ok((
                GameMove {
                    kind: MoveKind::Null,
                    san: "--".into(),
                },
                after,
            ))
        }
    }
}

/// Every move of the subtree of `source` from `s` can be made from `pos`.
fn check(source: &Game, s: NodeId, pos: &Chess) -> Result<(), EditError> {
    let mut stack = vec![(s, pos.clone())];
    while let Some((id, pos)) = stack.pop() {
        let mv = source.game_move(id).ok_or(EditError::UnknownNode)?;
        let (_, after) = play(&pos, mv)?;
        for &c in source.children(id) {
            stack.push((c, after.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(pgn: &str) -> Game {
        parse_str(pgn).game
    }

    /// The node reached by the SAN moves from the root (along any branch).
    fn at(g: &Game, sans: &[&str]) -> NodeId {
        let mut cur = NodeId::ROOT;
        for s in sans {
            cur = *g
                .children(cur)
                .iter()
                .find(|&&c| g.game_move(c).is_some_and(|m| m.san == *s))
                .unwrap_or_else(|| panic!("no {s}"));
        }
        cur
    }

    fn sans(p: &PastePlan) -> Vec<&str> {
        p.line.iter().map(|(_, _, s)| s.as_str()).collect()
    }

    const SICILIAN: &str = "1. e4 c5 2. Nf3 d6 3. d4 *";

    #[test]
    fn a_game_hangs_where_it_parts_and_undoes_whole() {
        let mut g = game(SICILIAN);
        let before = g.canonical_tree();
        let cursor = at(&g, &["e4", "c5", "Nf3", "d6", "d4"]);
        let p = plan(&g, cursor, "1.e4 c5 2.Nf3 Nc6 3.d4").unwrap();
        assert_eq!(p.at, at(&g, &["e4", "c5", "Nf3"]));
        assert_eq!(sans(&p), ["Nc6", "d4"]);
        assert!(!p.continuation);
        let first = g.apply_paste(&p).unwrap();
        assert_eq!(g.game_move(first).unwrap().san, "Nc6");
        // The main line is the same; the variation is the last child.
        assert_eq!(g.mainline().len(), 5);
        assert_eq!(g.children(p.at).last(), Some(&first));
        g.undo();
        assert_eq!(g.canonical_tree(), before);
    }

    #[test]
    fn an_unrelated_game_is_refused() {
        let g = game(SICILIAN);
        let cursor = at(&g, &["e4", "c5"]);
        assert_eq!(
            plan(&g, cursor, "1.d4 d5 2.c4").unwrap_err(),
            PasteError::Unrelated
        );
        assert_eq!(
            plan(&g, cursor, "[White \"A\"]\n[Black \"B\"]\n\n1.d4 d5 2.c4 *").unwrap_err(),
            PasteError::Unrelated
        );
    }

    #[test]
    fn another_move_order_is_found_and_noted() {
        let g = game(SICILIAN);
        let cursor = at(&g, &["e4", "c5", "Nf3"]);
        // All there by another order.
        assert!(matches!(
            plan(&g, cursor, "1.Nf3 c5 2.e4 d6 3.d4").unwrap_err(),
            PasteError::AlreadyThere { .. }
        ));
        let p = plan(&g, cursor, "1.Nf3 c5 2.e4 Nc6").unwrap();
        assert_eq!(p.at, at(&g, &["e4", "c5", "Nf3"]));
        assert_eq!(sans(&p), ["Nc6"]);
        assert_eq!(p.transposition.as_deref(), Some("1.Nf3 c5 2.e4"));
    }

    #[test]
    fn fragments_after_instead_of_or_by_number() {
        let g = game("1. e4 c6 2. d4 d5 3. Nc3 dxe4 4. Nxe4 Nf6 5. f3 Bf5 6. Ng3 *");
        let bf5 = at(
            &g,
            &[
                "e4", "c6", "d4", "d5", "Nc3", "dxe4", "Nxe4", "Nf6", "f3", "Bf5",
            ],
        );
        // Instead of the selected move.
        let p = plan(&g, bf5, "5...Nxe4 6.fxe4").unwrap();
        assert_eq!(p.at, g.parent(bf5).unwrap());
        assert_eq!(sans(&p), ["Nxe4", "fxe4"]);
        // After it.
        let p = plan(&g, bf5, "6.Qd3 e6").unwrap();
        assert_eq!(p.at, bf5);
        // Without numbers.
        let p = plan(&g, bf5, "Qd3 e6").unwrap();
        assert_eq!(p.at, bf5);
        // By the number on the current line.
        let e4 = at(&g, &["e4"]);
        let p = plan(&g, e4, "4...e5").unwrap();
        assert_eq!(
            p.at,
            at(&g, &["e4", "c6", "d4", "d5", "Nc3", "dxe4", "Nxe4"])
        );
        // Nowhere.
        assert_eq!(plan(&g, bf5, "Ke3").unwrap_err(), PasteError::DoesNotFit);
    }

    #[test]
    fn uci_lines_and_impossible_moves() {
        let g = Game::new();
        let p = plan(&g, NodeId::ROOT, "e2e4 e7e5 g1f3").unwrap();
        assert_eq!(sans(&p), ["e4", "e5", "Nf3"]);
        assert!(p.continuation);
        assert_eq!(
            plan(&g, NodeId::ROOT, "e2e4 e7e5 e1e3").unwrap_err(),
            PasteError::Illegal {
                number: 2,
                side: Color::White,
                san: "e1e3".into()
            }
        );
        let s = game(SICILIAN);
        assert_eq!(
            plan(&s, NodeId::ROOT, "1.e4 c5 2.Nf3 Nc6 3.Qh8").unwrap_err(),
            PasteError::Illegal {
                number: 3,
                side: Color::White,
                san: "Qh8".into()
            }
        );
    }

    #[test]
    fn comments_variations_and_the_source_come_along() {
        let mut g = game(SICILIAN);
        let text = "[White \"Mellado Trivino,J\"]\n[Black \"Garcia Albarracin,F\"]\n[Result \"0-1\"]\n\n\
                    1.e4 c5 2.Nf3 Nc6 {Old.} 3.d4 $1 (3.Bb5 g6) cxd4 0-1";
        let p = plan(&g, NodeId::ROOT, text).unwrap();
        assert_eq!(
            p.source_ref.as_deref(),
            Some("Mellado Trivino,J – Garcia Albarracin,F")
        );
        let first = g.apply_paste(&p).unwrap();
        let after = g.to_pgn();
        assert!(after.contains("{Old.}"), "{after}");
        assert!(after.contains("Bb5"), "{after}");
        assert_eq!(
            g.node(g.line_end(first)).unwrap().comments_after(),
            ["Mellado Trivino,J – Garcia Albarracin,F"]
        );
        assert_eq!(
            g.node(at(&g, &["e4", "c5", "Nf3", "Nc6", "d4"]))
                .unwrap()
                .nags()
                .len(),
            1
        );
        // Written and read again: the same tree.
        let again = game(&after);
        assert_eq!(again.canonical_tree(), g.canonical_tree());
        assert!(g.is_attached(first));
    }

    #[test]
    fn the_pasted_games_variations_come_along() {
        let mut g = game(SICILIAN);
        let before = g.canonical_tree();
        // An alternative at the parting point and one on a move in common.
        let text =
            "[White \"A\"]\n[Black \"B\"]\n\n1.e4 c5 (1...e5 2.Nf3) 2.Nf3 Nc6 (2...e6 3.d4) 3.d4 *";
        let p = plan(&g, NodeId::ROOT, text).unwrap();
        g.apply_paste(&p).unwrap();
        let nf3 = at(&g, &["e4", "c5", "Nf3"]);
        let kids = |g: &Game, id| -> Vec<String> {
            g.children(id)
                .iter()
                .map(|&c| g.game_move(c).unwrap().san.clone())
                .collect()
        };
        assert_eq!(kids(&g, nf3), ["d6", "Nc6", "e6"]);
        assert_eq!(kids(&g, at(&g, &["e4"])), ["c5", "e5"]);
        // One undo step for all of it.
        g.undo();
        assert_eq!(g.canonical_tree(), before);
        // Only a variation is new: it is pasted, not «already there».
        let p = plan(&g, NodeId::ROOT, "1.e4 c5 (1...e6) 2.Nf3 d6 3.d4").unwrap();
        assert_eq!(p.at, at(&g, &["e4"]));
        assert_eq!(sans(&p), ["e6"]);
    }

    #[test]
    fn black_numbers_with_an_ellipsis_character() {
        let g = game(SICILIAN);
        let p = plan(&g, at(&g, &["e4"]), "1…e5 2.Nf3").unwrap();
        assert_eq!(sans(&p), ["e5", "Nf3"]);
    }

    #[test]
    fn the_same_game_or_line_is_already_there() {
        let g = game(SICILIAN);
        match plan(&g, NodeId::ROOT, SICILIAN).unwrap_err() {
            PasteError::AlreadyThere { at: end, comments } => {
                assert_eq!(end, *g.mainline().last().unwrap());
                assert!(!comments);
            }
            e => panic!("{e:?}"),
        }
        assert!(matches!(
            plan(&g, NodeId::ROOT, "1.e4 c5 {Sicilian}").unwrap_err(),
            PasteError::AlreadyThere { comments: true, .. }
        ));
        assert_eq!(
            plan(&g, NodeId::ROOT, "  ").unwrap_err(),
            PasteError::NoMoves
        );
    }

    #[test]
    fn a_game_from_a_position_with_fullmove_zero() {
        let fen = "4k3/8/8/8/8/8/4P3/4K3 w - - 0 0";
        let mut g = game(&format!("[SetUp \"1\"]\n[FEN \"{fen}\"]\n\n1. e4 Kd7 *"));
        let e4 = at(&g, &["e4"]);
        let p = plan(&g, e4, "1...Ke7").unwrap();
        assert_eq!(p.at, e4);
        g.apply_paste(&p).unwrap();
        assert_eq!(g.children(e4).len(), 2);
    }

    #[test]
    fn several_games_paste_the_first() {
        let g = game(SICILIAN);
        let text = "[White \"A\"]\n\n1.e4 c5 2.Nf3 Nc6 *\n\n[White \"B\"]\n\n1.e4 e5 *\n";
        let p = plan(&g, NodeId::ROOT, text).unwrap();
        assert_eq!(p.games, 2);
        assert_eq!(sans(&p), ["Nc6"]);
    }
}
