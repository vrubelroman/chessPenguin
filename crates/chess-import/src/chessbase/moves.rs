//! `.cbg` game records: the start position and the move tree.
//!
//! Every byte of the move stream goes through a per-mode permutation together with
//! the number of moves decoded so far. The compact encoder then names a piece by
//! its index among the pieces of its kind (in the order they were found on the start
//! board) and gives a displacement modulo 8; the simple encoder stores squares.

use shakmaty::fen::Fen;
use shakmaty::san::SanPlus;
use shakmaty::{
    Bitboard, CastlingMode, Chess, Color, EnPassantMode, File, FromSetup, Move, Piece, Position,
    Rank, Role, Setup, Square,
};

use super::be;

/// The decoded move tree of one game.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    /// FEN of the start position when it is not the usual one.
    pub start_fen: Option<String>,
    /// `nodes[0]` is the start; every other node is a move.
    pub nodes: Vec<Node>,
    /// Node of each move in stored order (the annotation position numbering).
    pub order: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Node {
    /// SAN with check marks, or `--` for a null move.
    pub san: String,
    /// Move number and whether white made the move.
    pub number: u32,
    pub white: bool,
    /// The first child is the main continuation.
    pub children: Vec<usize>,
}

impl Tree {
    /// The start node alone: a game without moves.
    pub fn start() -> Tree {
        Tree {
            nodes: vec![Node::default()],
            ..Tree::default()
        }
    }
}

/// A game that could only be read in part.
pub struct DecodeError {
    pub partial: Tree,
    pub reason: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoder {
    Compact,
    Simple,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Modifier {
    Pre,
    Post,
}

pub fn decode(cbg: &[u8], offset: u32) -> Result<Tree, DecodeError> {
    let fail = |reason: &str| DecodeError {
        // The game is kept with its tags and no moves.
        partial: Tree::start(),
        reason: reason.into(),
    };
    let at = offset as usize;
    let (Some(flags), Some(size)) = (cbg.get(at).copied(), be(cbg, at + 1, 3)) else {
        return Err(fail("moves outside the .cbg file"));
    };
    let record = cbg
        .get(at..at + size as usize)
        .filter(|r| r.len() >= 4)
        .ok_or_else(|| fail("moves outside the .cbg file"))?;
    let mode = flags & 0x3f;
    let (encoder, modifier, table) = match mode {
        0 => (Encoder::Compact, Modifier::Pre, &TABLE_0),
        4 => (Encoder::Compact, Modifier::Post, &TABLE_4),
        5 => (Encoder::Simple, Modifier::Post, &TABLE_5),
        10 | 11 => return Err(fail("Chess960 games are not supported")),
        8 | 9 => return Err(fail("losing chess games are not supported")),
        m => return Err(fail(&format!("move encoding {m} is not supported"))),
    };
    let mut body = &record[4..];
    let (start, start_fen) = if flags & 0x40 != 0 {
        let s = body
            .get(..28)
            .ok_or_else(|| fail("truncated start position"))?;
        body = &body[28..];
        let pos = start_position(s).ok_or_else(|| fail("invalid start position"))?;
        let fen = Fen::from_position(&pos, EnPassantMode::Legal).to_string();
        (pos, Some(fen))
    } else {
        (Chess::default(), None)
    };
    let mut d = Decoder {
        tree: Tree {
            start_fen,
            nodes: vec![Node::default()],
            order: Vec::new(),
        },
        table,
        modifier,
        count: 0,
    };
    let result = match encoder {
        Encoder::Compact => d.compact(body, start),
        Encoder::Simple => d.simple(body, start),
    };
    match result {
        Ok(()) => Ok(d.tree),
        Err(reason) => Err(DecodeError {
            partial: d.tree,
            reason,
        }),
    }
}

/// The 28-byte start position: marker, en passant file and side to move, castling
/// rights, move number, then 192 bits of board, square by square from a1, a2, … h8.
fn start_position(s: &[u8]) -> Option<Chess> {
    let mut setup = Setup::empty();
    let mut bits = BitReader {
        data: &s[4..],
        pos: 0,
    };
    for idx in 0..64 {
        if bits.read(1)? == 0 {
            continue;
        }
        let color = if bits.read(1)? == 0 {
            Color::White
        } else {
            Color::Black
        };
        let role = match bits.read(3)? {
            1 => Role::King,
            2 => Role::Queen,
            3 => Role::Knight,
            4 => Role::Bishop,
            5 => Role::Rook,
            6 => Role::Pawn,
            _ => return None,
        };
        setup
            .board
            .set_piece_at(cb_square(idx), Piece { color, role });
    }
    setup.turn = if s[1] & 0x10 != 0 {
        Color::Black
    } else {
        Color::White
    };
    let mut rights = Bitboard::EMPTY;
    for (bit, sq) in [
        (0, Square::A1),
        (1, Square::H1),
        (2, Square::A8),
        (3, Square::H8),
    ] {
        if s[2] & (1 << bit) != 0 {
            rights.add(sq);
        }
    }
    setup.castling_rights = rights;
    let ep = u32::from(s[1] & 0x0f);
    if (1..=8).contains(&ep) {
        let rank = if setup.turn == Color::White {
            Rank::Sixth
        } else {
            Rank::Third
        };
        setup.ep_square = Some(Square::from_coords(File::new(ep - 1), rank));
    }
    setup.fullmoves = std::num::NonZeroU32::new(u32::from(s[3]).max(1))?;
    // Positions with doubtful castling rights or en passant squares are still usable.
    let pos = match Chess::from_setup(setup, CastlingMode::Standard) {
        Ok(p) => p,
        Err(e) => match e.ignore_invalid_castling_rights() {
            Ok(p) => p,
            Err(e) => e.ignore_invalid_ep_square().ok()?,
        },
    };
    Some(pos)
}

/// ChessBase numbers squares file by file: a1 = 0, a2 = 1, … h8 = 63.
fn cb_square(idx: u32) -> Square {
    Square::from_coords(File::new(idx / 8), Rank::new(idx % 8))
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl BitReader<'_> {
    fn read(&mut self, n: usize) -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            let byte = *self.data.get(self.pos / 8)?;
            v = (v << 1) | u32::from((byte >> (7 - self.pos % 8)) & 1);
            self.pos += 1;
        }
        Some(v)
    }
}

/// Which square each piece of a side is on, in ChessBase's numbering.
#[derive(Clone, Default)]
struct Side {
    king: Option<Square>,
    queens: Vec<Square>,
    rooks: Vec<Square>,
    bishops: Vec<Square>,
    knights: Vec<Square>,
    /// Pawns keep their number for the whole game; a captured pawn leaves a gap.
    pawns: [Option<Square>; 8],
}

#[derive(Clone)]
struct State {
    pos: Chess,
    sides: [Side; 2],
    node: usize,
}

impl State {
    fn new(pos: Chess) -> State {
        let mut sides: [Side; 2] = Default::default();
        let mut pawn_count = [0usize; 2];
        for idx in 0..64 {
            let sq = cb_square(idx);
            let Some(p) = pos.board().piece_at(sq) else {
                continue;
            };
            let c = p.color as usize;
            let s = &mut sides[c];
            match p.role {
                Role::King => s.king = Some(sq),
                Role::Queen => s.queens.push(sq),
                Role::Rook => s.rooks.push(sq),
                Role::Bishop => s.bishops.push(sq),
                Role::Knight => s.knights.push(sq),
                Role::Pawn => {
                    if let Some(slot) = s.pawns.get_mut(pawn_count[c]) {
                        *slot = Some(sq);
                    }
                    pawn_count[c] += 1;
                }
            }
        }
        State {
            pos,
            sides,
            node: 0,
        }
    }

    fn side(&mut self, c: Color) -> &mut Side {
        &mut self.sides[c as usize]
    }

    /// Keeps the piece lists in step with a move about to be played.
    fn track(&mut self, m: &Move) {
        let us = self.pos.turn();
        match *m {
            Move::Normal {
                from,
                to,
                capture,
                promotion,
                ..
            } => {
                if capture.is_some() {
                    self.side(!us).remove(to);
                }
                self.side(us).relocate(from, to);
                if let Some(role) = promotion {
                    let s = self.side(us);
                    s.remove(to);
                    s.list(role).push(to);
                }
            }
            Move::EnPassant { from, to } => {
                self.side(!us)
                    .remove(Square::from_coords(to.file(), from.rank()));
                self.side(us).relocate(from, to);
            }
            Move::Castle { king, rook } => {
                let short = rook.file() > king.file();
                let rank = king.rank();
                let (k, r) = if short {
                    (File::G, File::F)
                } else {
                    (File::C, File::D)
                };
                let s = self.side(us);
                s.relocate(rook, Square::from_coords(r, rank));
                s.relocate(king, Square::from_coords(k, rank));
            }
            Move::Put { .. } => {}
        }
    }
}

impl Side {
    fn list(&mut self, role: Role) -> &mut Vec<Square> {
        match role {
            Role::Queen => &mut self.queens,
            Role::Rook => &mut self.rooks,
            Role::Bishop => &mut self.bishops,
            _ => &mut self.knights,
        }
    }

    /// A captured piece: later pieces of its kind move down one; a pawn leaves a gap.
    fn remove(&mut self, sq: Square) {
        for list in [
            &mut self.queens,
            &mut self.rooks,
            &mut self.bishops,
            &mut self.knights,
        ] {
            if let Some(i) = list.iter().position(|s| *s == sq) {
                list.remove(i);
                return;
            }
        }
        for p in &mut self.pawns {
            if *p == Some(sq) {
                *p = None;
                return;
            }
        }
    }

    fn relocate(&mut self, from: Square, to: Square) {
        if self.king == Some(from) {
            self.king = Some(to);
            return;
        }
        for list in [
            &mut self.queens,
            &mut self.rooks,
            &mut self.bishops,
            &mut self.knights,
        ] {
            if let Some(s) = list.iter_mut().find(|s| **s == from) {
                *s = to;
                return;
            }
        }
        for p in self.pawns.iter_mut().flatten() {
            if *p == from {
                *p = to;
                return;
            }
        }
    }
}

struct Decoder {
    tree: Tree,
    table: &'static [u8; 256],
    modifier: Modifier,
    /// Moves decoded so far, variations and null moves included.
    count: u32,
}

/// What a byte of the compact encoder asks for.
enum Code {
    Null,
    /// Piece kind, its index, displacement.
    Piece(Role, usize, (i32, i32)),
    Pawn(usize, u8),
    Castle {
        short: bool,
    },
    TwoBytes,
    Ignore,
    Push,
    Pop,
}

const KING: [(i32, i32); 8] = [
    (0, 1),
    (1, 1),
    (1, 0),
    (1, -1),
    (0, -1),
    (-1, -1),
    (-1, 0),
    (-1, 1),
];
const KNIGHT: [(i32, i32); 8] = [
    (2, 1),
    (1, 2),
    (-1, 2),
    (-2, 1),
    (-2, -1),
    (-1, -2),
    (1, -2),
    (2, -1),
];

/// Displacement of the `i`-th step of a line piece: groups of 7 steps in the
/// directions `dirs`.
fn line_step(i: usize, dirs: &[(i32, i32)]) -> (i32, i32) {
    let (dx, dy) = dirs[i / 7];
    let n = (i % 7 + 1) as i32;
    (dx * n, dy * n)
}

const QUEEN_DIRS: [(i32, i32); 4] = [(0, 1), (1, 0), (1, 1), (1, -1)];
const ROOK_DIRS: [(i32, i32); 2] = [(0, 1), (1, 0)];
const BISHOP_DIRS: [(i32, i32); 2] = [(1, 1), (1, -1)];

fn classify(code: u8) -> Option<Code> {
    let c = usize::from(code);
    Some(match c {
        0 => Code::Null,
        1..=8 => Code::Piece(Role::King, 0, KING[c - 1]),
        9 => Code::Castle { short: true },
        10 => Code::Castle { short: false },
        11..=38 => Code::Piece(Role::Queen, 0, line_step(c - 11, &QUEEN_DIRS)),
        39..=52 => Code::Piece(Role::Rook, 0, line_step(c - 39, &ROOK_DIRS)),
        53..=66 => Code::Piece(Role::Rook, 1, line_step(c - 53, &ROOK_DIRS)),
        67..=80 => Code::Piece(Role::Bishop, 0, line_step(c - 67, &BISHOP_DIRS)),
        81..=94 => Code::Piece(Role::Bishop, 1, line_step(c - 81, &BISHOP_DIRS)),
        95..=102 => Code::Piece(Role::Knight, 0, KNIGHT[c - 95]),
        103..=110 => Code::Piece(Role::Knight, 1, KNIGHT[c - 103]),
        111..=142 => Code::Pawn((c - 111) / 4, ((c - 111) % 4) as u8),
        143..=170 => Code::Piece(Role::Queen, 1, line_step(c - 143, &QUEEN_DIRS)),
        171..=198 => Code::Piece(Role::Queen, 2, line_step(c - 171, &QUEEN_DIRS)),
        199..=212 => Code::Piece(Role::Rook, 2, line_step(c - 199, &ROOK_DIRS)),
        213..=226 => Code::Piece(Role::Bishop, 2, line_step(c - 213, &BISHOP_DIRS)),
        227..=234 => Code::Piece(Role::Knight, 2, KNIGHT[c - 227]),
        235 => Code::TwoBytes,
        236 => Code::Ignore,
        254 => Code::Push,
        255 => Code::Pop,
        _ => return None,
    })
}

impl Decoder {
    fn translate(&self, byte: u8) -> u8 {
        let n = (self.count % 256) as u8;
        match self.modifier {
            Modifier::Pre => self.table[usize::from(byte.wrapping_sub(n))],
            Modifier::Post => self.table[usize::from(byte)].wrapping_sub(n),
        }
    }

    fn compact(&mut self, data: &[u8], start: Chess) -> Result<(), String> {
        let mut state = State::new(start);
        let mut stack: Vec<State> = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let raw = self.translate(data[i]);
            i += 1;
            let code = classify(raw).ok_or_else(|| format!("unknown move code {raw}"))?;
            let m = match code {
                Code::Push => {
                    stack.push(state.clone());
                    continue;
                }
                Code::Pop => match stack.pop() {
                    Some(s) => {
                        state = s;
                        continue;
                    }
                    None => return Ok(()),
                },
                Code::Ignore => continue,
                Code::Null => None,
                Code::TwoBytes => {
                    let (Some(&a), Some(&b)) = (data.get(i), data.get(i + 1)) else {
                        return Err("truncated move".into());
                    };
                    i += 2;
                    let v = (u16::from(self.translate(a)) << 8) | u16::from(self.translate(b));
                    Some(square_move(&state.pos, v)?)
                }
                Code::Castle { short } => Some(castle(&state.pos, short)?),
                Code::Piece(role, index, delta) => {
                    let side = &state.sides[state.pos.turn() as usize];
                    let from = match role {
                        Role::King => side.king,
                        Role::Queen => side.queens.get(index).copied(),
                        Role::Rook => side.rooks.get(index).copied(),
                        Role::Bishop => side.bishops.get(index).copied(),
                        _ => side.knights.get(index).copied(),
                    }
                    .ok_or("move of a piece that is not on the board")?;
                    Some(find_move(&state.pos, from, displaced(from, delta), None)?)
                }
                Code::Pawn(index, kind) => {
                    let from = state.sides[state.pos.turn() as usize].pawns[index]
                        .ok_or("move of a pawn that is not on the board")?;
                    let white = state.pos.turn() == Color::White;
                    let delta = match (kind, white) {
                        (0, true) => (0, 1),
                        (1, true) => (0, 2),
                        (2, true) => (1, 1),
                        (_, true) => (-1, 1),
                        (0, false) => (0, -1),
                        (1, false) => (0, -2),
                        (2, false) => (-1, -1),
                        (_, false) => (1, -1),
                    };
                    Some(find_move(&state.pos, from, displaced(from, delta), None)?)
                }
            };
            self.play(&mut state, m)?;
        }
        // A stream without its final end marker: keep what was read.
        Ok(())
    }

    fn simple(&mut self, data: &[u8], start: Chess) -> Result<(), String> {
        let mut state = State::new(start);
        let mut stack: Vec<State> = Vec::new();
        for pair in data.as_chunks::<2>().0 {
            let v = (u16::from(self.translate(pair[0])) << 8) | u16::from(self.translate(pair[1]));
            if v & 0x8000 != 0 {
                stack.push(state.clone());
            }
            // A move from a square to itself is a null move.
            let m = if (v & 63) == ((v >> 6) & 63) {
                None
            } else {
                Some(square_move(&state.pos, v & 0x3fff)?)
            };
            self.play(&mut state, m)?;
            if v & 0x4000 != 0 {
                match stack.pop() {
                    Some(s) => state = s,
                    None => return Ok(()),
                }
            }
        }
        Ok(())
    }

    /// Adds the move (`None` = null move) as a child of the current node and plays it.
    fn play(&mut self, state: &mut State, m: Option<Move>) -> Result<(), String> {
        let pos = &state.pos;
        let node = Node {
            san: match m {
                Some(m) => SanPlus::from_move(pos.clone(), m).to_string(),
                None => "--".into(),
            },
            number: pos.fullmoves().get(),
            white: pos.turn() == Color::White,
            children: Vec::new(),
        };
        let next = match m {
            Some(m) => {
                state.track(&m);
                let mut p = state.pos.clone();
                p.play_unchecked(m);
                p
            }
            None => null_move(&state.pos).ok_or("null move while in check")?,
        };
        let id = self.tree.nodes.len();
        self.tree.nodes.push(node);
        self.tree.nodes[state.node].children.push(id);
        self.tree.order.push(id);
        state.node = id;
        state.pos = next;
        self.count += 1;
        Ok(())
    }
}

fn displaced(from: Square, (dx, dy): (i32, i32)) -> Square {
    let x = (from.file() as i32 + dx).rem_euclid(8) as u32;
    let y = (from.rank() as i32 + dy).rem_euclid(8) as u32;
    Square::from_coords(File::new(x), Rank::new(y))
}

/// Move given by squares: bits 0-5 from, 6-11 to, 12-13 promotion piece.
fn square_move(pos: &Chess, v: u16) -> Result<Move, String> {
    let from = cb_square(u32::from(v & 63));
    let to = cb_square(u32::from((v >> 6) & 63));
    let promotion =
        [Role::Queen, Role::Rook, Role::Bishop, Role::Knight][usize::from((v >> 12) & 3)];
    find_move(pos, from, to, Some(promotion))
}

/// The legal move from `from` to `to` (the king's destination for castling); a
/// promotion takes `promotion`, or a queen.
fn find_move(
    pos: &Chess,
    from: Square,
    to: Square,
    promotion: Option<Role>,
) -> Result<Move, String> {
    let wanted = promotion.unwrap_or(Role::Queen);
    pos.legal_moves()
        .into_iter()
        .find(|m| {
            m.from() == Some(from)
                && king_target(m) == to
                && m.promotion().is_none_or(|p| p == wanted)
        })
        .ok_or_else(|| "illegal move".to_owned())
}

fn castle(pos: &Chess, short: bool) -> Result<Move, String> {
    pos.legal_moves()
        .into_iter()
        .find(|m| matches!(m, Move::Castle { king, rook } if (rook.file() > king.file()) == short))
        .ok_or_else(|| "illegal castling".to_owned())
}

/// Destination as a player sees it: castling goes to the g or c file.
fn king_target(m: &Move) -> Square {
    match *m {
        Move::Castle { king, rook } => {
            let file = if rook.file() > king.file() {
                File::G
            } else {
                File::C
            };
            Square::from_coords(file, king.rank())
        }
        _ => m.to(),
    }
}

/// Teaching null move: the side to move passes (as `--` in PGN).
fn null_move(pos: &Chess) -> Option<Chess> {
    if pos.is_check() {
        return None;
    }
    let mut setup = pos.to_setup(EnPassantMode::Legal);
    let was_black = setup.turn == Color::Black;
    setup.swap_turn();
    setup.ep_square = None;
    setup.halfmoves = setup.halfmoves.saturating_add(1);
    if was_black {
        setup.fullmoves = setup.fullmoves.saturating_add(1);
    }
    Chess::from_setup(setup, CastlingMode::Standard).ok()
}

const TABLE_0: [u8; 256] = [
    0xa2, 0x95, 0x43, 0xf5, 0xc1, 0x3d, 0x4a, 0x6c, 0x53, 0x83, 0xcc, 0x7c, 0xff, 0xae, 0x68, 0xad,
    0xd1, 0x92, 0x8b, 0x8d, 0x35, 0x81, 0x5e, 0x74, 0x26, 0x8e, 0xab, 0xca, 0xfd, 0x9a, 0xf3, 0xa0,
    0xa5, 0x15, 0xfc, 0xb1, 0x1e, 0xed, 0x30, 0xea, 0x22, 0xeb, 0xa7, 0xcd, 0x4e, 0x6f, 0x2e, 0x24,
    0x32, 0x94, 0x41, 0x8c, 0x6e, 0x58, 0x82, 0x50, 0xbb, 0x02, 0x8a, 0xd8, 0xfa, 0x60, 0xde, 0x52,
    0xba, 0x46, 0xac, 0x29, 0x9d, 0xd7, 0xdf, 0x08, 0x21, 0x01, 0x66, 0xa3, 0xf1, 0x19, 0x27, 0xb5,
    0x91, 0xd5, 0x42, 0x0e, 0xb4, 0x4c, 0xd9, 0x18, 0x5f, 0xbc, 0x25, 0xa6, 0x96, 0x04, 0x56, 0x6a,
    0xaa, 0x33, 0x1c, 0x2b, 0x73, 0xf0, 0xdd, 0xa4, 0x37, 0xd3, 0xc5, 0x10, 0xbf, 0x5a, 0x23, 0x34,
    0x75, 0x5b, 0xb8, 0x55, 0xd2, 0x6b, 0x09, 0x3a, 0x57, 0x12, 0xb3, 0x77, 0x48, 0x85, 0x9b, 0x0f,
    0x9e, 0xc7, 0xc8, 0xa1, 0x7f, 0x7a, 0xc0, 0xbd, 0x31, 0x6d, 0xf6, 0x3e, 0xc3, 0x11, 0x71, 0xce,
    0x7d, 0xda, 0xa8, 0x54, 0x90, 0x97, 0x1f, 0x44, 0x40, 0x16, 0xc9, 0xe3, 0x2c, 0xcb, 0x84, 0xec,
    0x9f, 0x3f, 0x5c, 0xe6, 0x76, 0x0b, 0x3c, 0x20, 0xb7, 0x36, 0x00, 0xdc, 0xe7, 0xf9, 0x4f, 0xf7,
    0xaf, 0x06, 0x07, 0xe0, 0x1a, 0x0a, 0xa9, 0x4b, 0x0c, 0xd6, 0x63, 0x87, 0x89, 0x1d, 0x13, 0x1b,
    0xe4, 0x70, 0x05, 0x47, 0x67, 0x7b, 0x2f, 0xee, 0xe2, 0xe8, 0x98, 0x0d, 0xef, 0xcf, 0xc4, 0xf4,
    0xfb, 0xb0, 0x17, 0x99, 0x64, 0xf2, 0xd4, 0x2a, 0x03, 0x4d, 0x78, 0xc6, 0xfe, 0x65, 0x86, 0x88,
    0x79, 0x45, 0x3b, 0xe5, 0x49, 0x8f, 0x2d, 0xb9, 0xbe, 0x62, 0x93, 0x14, 0xe9, 0xd0, 0x38, 0x9c,
    0xb2, 0xc2, 0x59, 0x5d, 0xb6, 0x72, 0x51, 0xf8, 0x28, 0x7e, 0x61, 0x39, 0xe1, 0xdb, 0x69, 0x80,
];

const TABLE_4: [u8; 256] = [
    0x58, 0xa3, 0xee, 0x72, 0x47, 0xb9, 0x1f, 0xa0, 0x26, 0xfd, 0xdc, 0x6d, 0xf1, 0xe3, 0xcb, 0xc5,
    0xe2, 0xd0, 0x29, 0xe7, 0x89, 0xe1, 0xae, 0x7b, 0x34, 0x41, 0x35, 0x4b, 0xa5, 0x7c, 0xf6, 0xe5,
    0xdb, 0x8c, 0x5f, 0x37, 0x05, 0xbb, 0x32, 0x54, 0x4c, 0xef, 0x5d, 0x2a, 0x7f, 0xd3, 0x81, 0x86,
    0xdf, 0x18, 0xc3, 0x44, 0x16, 0xf7, 0x85, 0x0f, 0x4d, 0x30, 0x79, 0x14, 0x45, 0xaa, 0xa2, 0x6c,
    0x15, 0x92, 0xa6, 0x62, 0x48, 0x19, 0xe9, 0x3c, 0x38, 0x94, 0x28, 0xa8, 0x66, 0x09, 0x03, 0x63,
    0x20, 0x7d, 0x36, 0xd7, 0x2e, 0x98, 0xec, 0xda, 0x01, 0x13, 0x24, 0x73, 0xcd, 0x9e, 0xcf, 0xd9,
    0xd5, 0x68, 0x8b, 0xd2, 0xab, 0x5b, 0x69, 0xb0, 0x27, 0xfc, 0x31, 0x1b, 0x53, 0x7e, 0x65, 0x76,
    0x55, 0x75, 0xaf, 0x0a, 0xcc, 0xc7, 0xc4, 0xb2, 0xb5, 0x22, 0xe8, 0xa7, 0xbe, 0xf5, 0x00, 0x5a,
    0x21, 0xd1, 0x74, 0x8e, 0xde, 0x9c, 0xe4, 0x42, 0x2b, 0xb1, 0x9b, 0x93, 0xc1, 0x78, 0x70, 0xd6,
    0xb6, 0xad, 0x4e, 0x6a, 0x64, 0x7a, 0x57, 0x49, 0x11, 0x9d, 0x12, 0xc0, 0x3a, 0x2f, 0xe6, 0x84,
    0xc6, 0x07, 0xce, 0x08, 0x82, 0x04, 0x90, 0xb8, 0x87, 0xca, 0x6e, 0x1e, 0x40, 0x1a, 0x50, 0xb4,
    0x10, 0xbc, 0x43, 0x8a, 0x71, 0x0b, 0x8d, 0x5c, 0x3b, 0x3f, 0x88, 0xba, 0xf4, 0xf3, 0x61, 0x1d,
    0xf9, 0xfe, 0x25, 0x51, 0x0d, 0xeb, 0xc8, 0x59, 0x17, 0x46, 0xa4, 0xf2, 0x56, 0xb7, 0x02, 0x2d,
    0x33, 0x9f, 0xd8, 0x83, 0xfb, 0xbf, 0xea, 0xed, 0xac, 0x60, 0x67, 0x39, 0xdd, 0x96, 0x6b, 0xa9,
    0xc2, 0x77, 0x0c, 0x0e, 0x91, 0xfa, 0x99, 0x9a, 0xe0, 0xa1, 0xf0, 0x5e, 0xc9, 0x06, 0x4f, 0xbd,
    0x52, 0x80, 0xd4, 0xf8, 0x97, 0x23, 0x3e, 0x1c, 0x2c, 0x95, 0x4a, 0x8f, 0x3d, 0x6f, 0xff, 0xb3,
];

const TABLE_5: [u8; 256] = [
    0xe0, 0x1b, 0x97, 0x86, 0xa2, 0x48, 0xf0, 0x23, 0x01, 0x37, 0xfa, 0xeb, 0x50, 0x76, 0x5d, 0xcd,
    0x80, 0x1a, 0x31, 0x5c, 0xff, 0xbc, 0x78, 0xda, 0xe5, 0xa3, 0x75, 0xb3, 0x71, 0xe1, 0xa1, 0x0e,
    0x56, 0x41, 0x3f, 0xe3, 0x0d, 0xc4, 0x25, 0xe6, 0x88, 0x89, 0x70, 0xdc, 0x99, 0xa6, 0x0b, 0x9a,
    0x14, 0x2c, 0x57, 0x9e, 0x69, 0x7a, 0xdf, 0x55, 0xad, 0x73, 0x7c, 0xc5, 0x03, 0x72, 0xf5, 0x45,
    0x62, 0x8e, 0xaf, 0x5f, 0xcc, 0xc0, 0x5e, 0xf3, 0x18, 0x13, 0xb5, 0x38, 0x09, 0x28, 0x3c, 0xbd,
    0x10, 0x8b, 0xe9, 0xdd, 0x8c, 0x94, 0x08, 0x81, 0xa7, 0xfe, 0x24, 0x79, 0x44, 0x2b, 0xc1, 0x36,
    0x32, 0x7f, 0x67, 0xe4, 0xfd, 0x49, 0x4b, 0x21, 0xa9, 0x5b, 0x77, 0x1f, 0x1c, 0xf6, 0x40, 0x65,
    0x3d, 0xf9, 0x5a, 0xf8, 0x47, 0x26, 0x53, 0xd4, 0x20, 0xd2, 0xd8, 0x4f, 0x27, 0xae, 0xfb, 0xb6,
    0x9b, 0x74, 0xb8, 0x42, 0xfc, 0x16, 0x6f, 0xd7, 0x9f, 0xb9, 0x7b, 0x60, 0xde, 0xc2, 0x46, 0x06,
    0x9c, 0xd3, 0x2a, 0xc7, 0x05, 0x85, 0x3e, 0xb1, 0x00, 0x15, 0xd1, 0x96, 0xce, 0x90, 0x95, 0x11,
    0x33, 0x8f, 0xc6, 0x82, 0x84, 0xb7, 0x39, 0xa0, 0xe8, 0x34, 0x1e, 0x7d, 0xa8, 0xd0, 0xea, 0xe2,
    0xef, 0x4c, 0xc3, 0x58, 0x4e, 0x02, 0x1d, 0xb2, 0x7e, 0x98, 0x2d, 0x07, 0xd5, 0xca, 0x68, 0xcb,
    0x30, 0xb4, 0xac, 0x83, 0x63, 0xec, 0x87, 0xee, 0x2e, 0xbe, 0xcf, 0x6d, 0x4d, 0x8a, 0xd6, 0x04,
    0xb0, 0x35, 0xc8, 0xc9, 0x12, 0x17, 0x61, 0x8d, 0x64, 0x0f, 0xa5, 0x54, 0x29, 0x3a, 0x4a, 0x6b,
    0xbb, 0x2f, 0xd9, 0x3b, 0x93, 0x6a, 0xaa, 0xa4, 0x22, 0xf4, 0x19, 0x43, 0x6e, 0xf1, 0xf7, 0xdb,
    0x92, 0xed, 0x66, 0x59, 0x0a, 0xbf, 0x6c, 0x51, 0x91, 0x52, 0xf2, 0x0c, 0x9d, 0xab, 0xba, 0xe7,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_permutations() {
        for t in [&TABLE_0, &TABLE_4, &TABLE_5] {
            let mut seen = [false; 256];
            for v in t.iter() {
                assert!(!seen[usize::from(*v)]);
                seen[usize::from(*v)] = true;
            }
        }
    }

    #[test]
    fn start_board_bits() {
        // The example from the format description: white pawn a2, black rook b1,
        // white knight b4 (bits 0 10110 000000 11101 00 10011), plus both kings.
        let mut s = vec![1, 0, 0, 1];
        let mut board = vec![0x58, 0x0e, 0x93];
        board.resize(24, 0);
        s.extend(board);
        // Kings are needed for a valid position: add them on h1 (white) and h8 (black).
        let mut bits: Vec<u8> = Vec::new();
        let mut pos = BitReader {
            data: &s[4..],
            pos: 0,
        };
        for _ in 0..24 {
            bits.push(pos.read(1).unwrap_or(0) as u8);
        }
        assert_eq!(bits[..6], [0, 1, 0, 1, 1, 0]);
        assert!(start_position(&s).is_none(), "no kings: invalid");
    }

    #[test]
    fn displacement_wraps_modulo_8() {
        // A queen on c2 moving (+7, 0) reaches b2.
        assert_eq!(displaced(Square::C2, (7, 0)), Square::B2);
        assert_eq!(displaced(Square::A1, (-1, -1)), Square::H8);
        assert_eq!(line_step(7, &QUEEN_DIRS), (1, 0));
        assert_eq!(line_step(13, &QUEEN_DIRS), (7, 0));
        assert_eq!(line_step(27, &QUEEN_DIRS), (7, -7));
    }

    #[test]
    fn unreadable_game_keeps_the_start_node() {
        // A Chess960 record (mode 10) of 4 bytes: rejected before any move, as in Mega 2026.
        let err = decode(&[10, 0, 0, 4], 0).expect_err("Chess960 is not supported");
        assert_eq!(err.partial.nodes.len(), 1);
        let mut out = String::new();
        let notes = super::super::annotations::Annotations::default();
        super::super::writer::game(&mut out, &Vec::new(), &err.partial, &notes, "*");
        assert_eq!(out.trim(), "*");
    }

    #[test]
    fn piece_numbering_follows_the_start_board() {
        let s = State::new(Chess::default());
        let w = &s.sides[Color::White as usize];
        assert_eq!(w.rooks, vec![Square::A1, Square::H1]);
        assert_eq!(w.knights, vec![Square::B1, Square::G1]);
        assert_eq!(w.pawns[0], Some(Square::A2));
        assert_eq!(w.pawns[7], Some(Square::H2));
        let b = &s.sides[Color::Black as usize];
        assert_eq!(b.rooks, vec![Square::A8, Square::H8]);
        assert_eq!(b.pawns[0], Some(Square::A7));
    }
}
