//! Synthetic PGN fixtures (written for this project, not taken from books).

use chess_core::shakmaty::{Color, Move, Position, Role, Square};
use chess_core::{
    DiagnosticCode, Game, GameResult, HistoryKind, MoveKind, Nag, NodeId, ParsedGame, Severity,
    TextEncoding, parse_game, parse_str,
};

fn parse_ok(pgn: &str) -> ParsedGame {
    let p = parse_str(pgn);
    for d in &p.diagnostics {
        eprintln!("{d}");
    }
    p
}

fn sans(g: &Game, ids: &[NodeId]) -> Vec<String> {
    ids.iter()
        .filter_map(|id| g.game_move(*id).map(|m| m.san.clone()))
        .collect()
}

fn child_sans(g: &Game, id: NodeId) -> Vec<String> {
    sans(g, g.children(id))
}

fn has(p: &ParsedGame, code: DiagnosticCode) -> bool {
    p.diagnostics.iter().any(|d| d.code == code)
}

/// import → export → import gives the same tree, tags and start; export is idempotent.
fn assert_roundtrip(p: &ParsedGame) -> String {
    let first = p.game.to_pgn();
    let again = parse_str(&first);
    assert_eq!(
        p.game.canonical_tree(),
        again.game.canonical_tree(),
        "tree changed on roundtrip:\n{first}"
    );
    assert_eq!(p.game.tags(), again.game.tags());
    assert_eq!(p.game.start().fen(), again.game.start().fen());
    assert_eq!(p.game.result(), again.game.result());
    assert_eq!(first, again.game.to_pgn(), "export not idempotent");
    first
}

const NESTED: &str = r#"[Event "Synthetic nested"]
[Site "?"]
[Date "2026.10.17"]
[Round "1"]
[White "Тестов, А."]
[Black "Test, B."]
[Result "1-0"]
[Annotator "fixture"]
[MyCustomTag "with \"quotes\" and \\ backslash"]

{Start comment} 1. e4 $1 {after e4} e5 (1... c5 {Sicilian} 2. Nf3 (2. Nc3 Nc6
(2... d6 3. f4 $5) 3. f4) 2... d6) ({Before French} 1... e6 $2 2. d4 d5) 2. Nf3
Nc6 3. Bb5 a6 $6 4. Ba4 Nf6 5. O-O Be7 $14 1-0
"#;

#[test]
fn nested_variations_comments_and_tags() {
    let p = parse_ok(NESTED);
    assert!(
        p.max_severity() <= Some(Severity::Info),
        "{:?}",
        p.diagnostics
    );
    let g = &p.game;
    assert_eq!(
        g.tag("MyCustomTag"),
        Some("with \"quotes\" and \\ backslash")
    );
    assert_eq!(g.tag("White"), Some("Тестов, А."));
    assert_eq!(g.tags()[7].0, "Annotator");
    assert_eq!(g.result(), GameResult::WhiteWins);

    let root = g.node(g.root()).expect("root");
    assert_eq!(root.comments_after(), &["Start comment".to_owned()]);

    let main = g.mainline();
    assert_eq!(
        sans(g, &main),
        [
            "e4", "e5", "Nf3", "Nc6", "Bb5", "a6", "Ba4", "Nf6", "O-O", "Be7"
        ]
    );
    let e4 = main[0];
    assert_eq!(g.node(e4).expect("e4").nags(), &[Nag(1)]);
    assert_eq!(
        g.node(e4).expect("e4").comments_after(),
        &["after e4".to_owned()]
    );
    assert_eq!(child_sans(g, e4), ["e5", "c5", "e6"]);

    let c5 = g.children(e4)[1];
    let e6 = g.children(e4)[2];
    assert_eq!(
        g.node(c5).expect("c5").comments_after(),
        &["Sicilian".to_owned()]
    );
    assert_eq!(
        g.node(e6).expect("e6").comments_before(),
        &["Before French".to_owned()]
    );
    assert_eq!(g.node(e6).expect("e6").nags(), &[Nag(2)]);
    assert_eq!(child_sans(g, c5), ["Nf3", "Nc3"]);
    let nc3 = g.children(c5)[1];
    assert_eq!(child_sans(g, nc3), ["Nc6", "d6"]);
    let d6 = g.children(nc3)[1];
    assert_eq!(g.variation_depth(d6), 3);
    let f4 = g.main_child(d6).expect("f4");
    assert_eq!(g.node(f4).expect("f4").nags(), &[Nag(5)]);
    assert_eq!(g.move_label(f4).as_deref(), Some("3. f4"));
    assert_eq!(g.move_label(d6).as_deref(), Some("2... d6"));
    assert_eq!(g.variation_start(f4), Some(d6));
    assert!(!g.is_mainline(f4));
    assert_eq!(g.path(f4), vec![g.root(), e4, c5, nc3, d6, f4]);
    assert_eq!(g.next_sibling(c5), Some(e6));
    assert_eq!(g.prev_sibling(c5), Some(main[1]));

    let castle = main[8];
    assert!(matches!(
        g.game_move(castle).map(|m| m.kind),
        Some(MoveKind::Normal(Move::Castle { .. }))
    ));
    let h = g.engine_history(castle).expect("history");
    assert_eq!(h.kind, HistoryKind::Complete);
    assert_eq!(h.start_fen, None);
    assert_eq!(h.moves.last().map(String::as_str), Some("e1g1"));
    assert_eq!(h.moves.len(), 9);

    let out = assert_roundtrip(&p);
    assert!(out.contains("[MyCustomTag \"with \\\"quotes\\\" and \\\\ backslash\"]"));
    assert!(out.lines().all(|l| l.chars().count() <= 80), "{out}");
}

#[test]
fn fen_black_to_move_fullmove_zero() {
    let pgn = "[Event \"?\"]\n[SetUp \"1\"]\n[FEN \"8/8/8/4k3/8/8/4P3/4K3 b - - 0 0\"]\n\n1... Kd5 2. e4+ Kxe4 *\n";
    let p = parse_ok(pgn);
    assert!(has(&p, DiagnosticCode::FenFullmoveZero));
    assert!(p.max_severity() <= Some(Severity::Info));
    let g = &p.game;
    assert_eq!(g.position(g.root()).expect("root").turn(), Color::Black);
    let main = g.mainline();
    assert_eq!(sans(g, &main), ["Kd5", "e4+", "Kxe4"]);
    assert_eq!(g.move_number(main[0]), Some((1, Color::Black)));
    assert_eq!(g.move_label(main[1]).as_deref(), Some("2. e4+"));
    let h = g.engine_history(main[2]).expect("h");
    assert_eq!(
        h.start_fen.as_deref(),
        Some("8/8/8/4k3/8/8/4P3/4K3 b - - 0 1")
    );
    assert_eq!(h.moves, ["e5d5", "e2e4", "d5e4"]);
    let out = assert_roundtrip(&p);
    assert!(out.contains("[FEN \"8/8/8/4k3/8/8/4P3/4K3 b - - 0 0\"]"));
    assert!(out.contains("1... Kd5 2. e4+ Kxe4 *"), "{out}");
}

#[test]
fn en_passant_and_promotions() {
    let pgn = "[FEN \"4k3/8/8/3pP3/8/8/1p6/4K3 w - d6 0 1\"]\n[SetUp \"1\"]\n\n1. exd6 b1=N (1... b1=Q+ 2. Kd2) 2. d7+ Kxd7 *\n";
    let p = parse_ok(pgn);
    let g = &p.game;
    let main = g.mainline();
    assert_eq!(sans(g, &main), ["exd6", "b1=N", "d7+", "Kxd7"]);
    assert!(matches!(
        g.game_move(main[0]).map(|m| m.kind),
        Some(MoveKind::Normal(Move::EnPassant { .. }))
    ));
    assert!(matches!(
        g.game_move(main[1]).map(|m| m.kind),
        Some(MoveKind::Normal(Move::Normal {
            promotion: Some(Role::Knight),
            ..
        }))
    ));
    let q = g.children(main[0])[1];
    assert_eq!(g.game_move(q).map(|m| m.san.as_str()), Some("b1=Q+"));
    assert_eq!(g.engine_history(q).expect("h").moves, ["e5d6", "b2b1q"]);
    assert_roundtrip(&p);
}

#[test]
fn teaching_null_moves() {
    let pgn = "[SetUp \"1\"]\n[FEN \"4k3/8/4K3/4P3/8/8/8/8 w - - 0 1\"]\n\n{[%csl Rd7,Rf7]} 1. -- Kd8 (1... Kf8 2. Kd7) (1... -- 2. Kf6) 2. Kf7 *\n";
    let p = parse_ok(pgn);
    assert!(p.max_severity() <= Some(Severity::Info));
    let g = &p.game;
    let main = g.mainline();
    assert_eq!(sans(g, &main), ["--", "Kd8", "Kf7"]);
    assert!(g.game_move(main[0]).expect("null").is_null());
    assert_eq!(g.position(main[0]).expect("p").turn(), Color::Black);
    let h = g.engine_history(main[2]).expect("h");
    assert_eq!(h.kind, HistoryKind::AfterNullMove { null_moves: 1 });
    assert_eq!(h.start_fen, g.fen(main[0]));
    assert_eq!(h.moves, ["e8d8", "e6f7"]);
    assert_eq!(h.current_fen, g.fen(main[2]).expect("fen"));
    let black_null = g.children(main[0])[2];
    assert!(g.game_move(black_null).expect("null").is_null());
    let kf6 = g.main_child(black_null).expect("Kf6");
    assert_eq!(g.move_number(kf6), Some((2, Color::White)));
    let h = g.engine_history(kf6).expect("h");
    assert_eq!(h.kind, HistoryKind::AfterNullMove { null_moves: 2 });
    assert_eq!(h.moves, ["e6f6"]);
    assert!(
        h.start_fen
            .as_deref()
            .is_some_and(|f| f.ends_with(" w - - 2 2"))
    );
    let out = assert_roundtrip(&p);
    assert!(
        out.contains("1. -- Kd8 (1... Kf8 2. Kd7) (1... -- 2. Kf6) 2. Kf7 *"),
        "{out}"
    );
}

#[test]
fn null_move_in_check_is_not_played() {
    let pgn = "[SetUp \"1\"]\n[FEN \"4k3/8/8/8/8/8/8/r3K3 w - - 0 1\"]\n\n1. -- Ra2 *\n";
    let p = parse_ok(pgn);
    assert!(has(&p, DiagnosticCode::IllegalMove));
    let root = p.game.node(p.game.root()).expect("root");
    assert_eq!(root.unparsed(), &["-- Ra2".to_owned()]);
    assert_eq!(p.game.result(), GameResult::Unknown);
    let out = assert_roundtrip(&p);
    assert!(out.contains("-- Ra2 *"));
}

#[test]
fn illegal_move_keeps_rest_of_branch() {
    let pgn =
        "[Result \"1-0\"]\n\n1. e4 (1. d4 Ke5 2. c4 (2. Nf3)) e5 2. Ke3 Nc6 (2... d6) 3. d4 1-0\n";
    let p = parse_ok(pgn);
    let errors: Vec<_> = p
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::IllegalMove)
        .collect();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].location.map(|l| l.line), Some(3));
    let g = &p.game;
    let main = g.mainline();
    assert_eq!(sans(g, &main), ["e4", "e5"]);
    let d4 = g.children(g.root())[1];
    assert_eq!(
        g.node(d4).expect("d4").unparsed(),
        &["Ke5 2. c4 (2. Nf3)".to_owned()]
    );
    assert_eq!(
        g.node(main[1]).expect("e5").unparsed(),
        &["Ke3 Nc6 (2... d6) 3. d4".to_owned()]
    );
    assert_eq!(g.result(), GameResult::WhiteWins);
    let out = assert_roundtrip(&p);
    assert!(out.contains("Ke3 Nc6 (2... d6) 3. d4 1-0"), "{out}");
}

#[test]
fn early_closed_comment_text_is_not_lost() {
    // Shape of a real defect: '}' closes the comment early, text continues
    // outside braces, then a stray '}'.
    let pgn = "[Event \"?\"]\n\n{Если слон не справляется (а так бывает чаще всего}, то исход\nборьбы зависит от короля.} {При короле на a1} 1. e4 *\n";
    let p = parse_ok(pgn);
    assert!(has(&p, DiagnosticCode::UnrecognizedText));
    assert!(has(&p, DiagnosticCode::StrayClosingBrace));
    let root = p.game.node(p.game.root()).expect("root");
    assert_eq!(
        root.comments_after(),
        &[
            "Если слон не справляется (а так бывает чаще всего".to_owned(),
            ", то исход борьбы зависит от короля.".to_owned(),
            "При короле на a1".to_owned(),
        ]
    );
    let d = p
        .diagnostics
        .iter()
        .find(|d| d.code == DiagnosticCode::UnrecognizedText)
        .expect("diag");
    assert_eq!(d.location.map(|l| l.line), Some(3));
    assert_eq!(d.node, Some(p.game.root()));
    assert_roundtrip(&p);
}

#[test]
fn symbolic_annotations_and_legacy_syntax() {
    let pgn = "[SetUp \"1\"]\n[FEN \"r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1\"]\n\n1.0-0! 0-0-0?! ; line comment\n%escaped line\n2.Rf7 +- $146 *";
    let p = parse_ok(pgn);
    let g = &p.game;
    let main = g.mainline();
    assert_eq!(sans(g, &main), ["O-O", "O-O-O", "Rf7"]);
    assert_eq!(g.node(main[0]).expect("n").nags(), &[Nag(1)]);
    assert_eq!(g.node(main[1]).expect("n").nags(), &[Nag(6)]);
    assert_eq!(
        g.node(main[1]).expect("n").comments_after(),
        &["line comment".to_owned()]
    );
    assert_eq!(g.node(main[2]).expect("n").nags(), &[Nag(18), Nag(146)]);
    assert!(has(&p, DiagnosticCode::EscapeLine));
    assert_eq!(
        g.engine_history(main[1]).expect("h").moves,
        ["e1g1", "e8c8"]
    );
    assert_roundtrip(&p);
}

#[test]
fn truncated_and_garbage_inputs_do_not_panic() {
    let cases: &[&[u8]] = &[
        b"",
        b"[Event \"x\"",
        b"[Event \"x\"]\n\n1. e4 {never closed",
        b"[Event \"x\"]\n\n1. e4 (1. d4 d5 (1... Nf6",
        b"1. e4 ) ) e5 *",
        b"( 1. e4 ) 1. d4 *",
        b"1. e4 () e5 *",
        b"[FEN \"8/8/8/8/8/8/8/8 w - - 0 1\"]\n\n1. e4 *",
        b"[FEN \"not a fen\"]\n\n1. e4 {c} *",
        b"\xff\xfe\x00garbage {\xc3\x28} 1. e4 $999 $ e5 ***",
        b"1. e4 1-0 0-1",
        b"1. e4 e5 2. Qh5 Nc6 3. Bc4 Nf6 4. Qxf7# 1-0 {after}",
        b"[White \"\xe0\xe1\"]\n\n1. e8=Q *",
    ];
    for c in cases {
        for enc in [TextEncoding::Utf8, TextEncoding::Windows1251] {
            let p = parse_game(c, enc);
            let _ = p.game.to_pgn();
            assert_roundtrip(&parse_str(&p.game.to_pgn()));
        }
    }
    let p = parse_game(b"[Event \"x\"]\n\n1. e4 {never closed", TextEncoding::Utf8);
    assert!(has(&p, DiagnosticCode::UnterminatedComment));
    assert_eq!(
        p.game
            .node(p.game.mainline()[0])
            .expect("e4")
            .comments_after(),
        &["never closed".to_owned()]
    );
    let p = parse_game(b"1. e4 (1. d4 d5 (1... Nf6", TextEncoding::Utf8);
    assert!(has(&p, DiagnosticCode::UnclosedVariation));
    assert_eq!(p.game.children(p.game.root()).len(), 2);
    let p = parse_game(b"[FEN \"not a fen\"]\n\n1. e4 {c} *", TextEncoding::Utf8);
    assert!(has(&p, DiagnosticCode::InvalidFen));
    assert_eq!(
        p.game.node(p.game.root()).expect("root").unparsed(),
        &["1. e4 {c}".to_owned()]
    );
    assert!(p.game.to_pgn().contains("[FEN \"not a fen\"]"));
    let p = parse_game(b"( 1. e4 ) 1. d4 *", TextEncoding::Utf8);
    assert!(has(&p, DiagnosticCode::VariationWithoutMove));
    assert_eq!(p.game.mainline().len(), 1);
}

#[test]
fn tags_only_record() {
    let p = parse_ok(
        "[Event \"Diagram\"]\n[SetUp \"1\"]\n[FEN \"8/8/8/8/8/8/k7/2K5 w - - 0 1\"]\n\n{Позиция без ходов} *\n",
    );
    assert!(p.game.mainline().is_empty());
    assert_eq!(
        p.game.node(p.game.root()).expect("r").comments_after(),
        &["Позиция без ходов".to_owned()]
    );
    assert_roundtrip(&p);
}

#[test]
fn edited_game_exports_and_reimports() {
    let mut g = Game::new();
    let root = g.root();
    let last = g
        .add_uci_line(root, &["e2e4", "e7e5", "g1f3", "b8c6"])
        .expect("line");
    let e4 = g.main_child(root).expect("e4");
    let pv_end = g.add_uci_line(e4, &["c7c5", "g1f3", "d7d6"]).expect("pv");
    assert_eq!(g.variation_depth(pv_end), 1);
    let ep_pos = g.position(last).expect("pos").clone();
    let m = Move::Normal {
        role: Role::Bishop,
        from: Square::F1,
        capture: None,
        to: Square::B5,
        promotion: None,
    };
    assert!(ep_pos.is_legal(m));
    g.add_move(last, m).expect("Bb5");
    g.set_result(GameResult::Draw);
    let text = g.to_pgn();
    assert!(
        text.contains("1. e4 e5 (1... c5 2. Nf3 d6) 2. Nf3 Nc6 3. Bb5 1/2-1/2"),
        "{text}"
    );
    let back = parse_str(&text);
    assert!(
        back.max_severity() <= Some(Severity::Info),
        "{:?}",
        back.diagnostics
    );
    assert_eq!(back.game.canonical_tree(), g.canonical_tree());
    assert_eq!(back.game.result(), GameResult::Draw);

    let from_fen = Game::from_start(
        chess_core::StartPosition::from_fen("4k3/8/8/8/8/8/4P3/4K3 b - - 3 40").expect("fen"),
    );
    let text = from_fen.to_pgn();
    assert!(
        text.contains("[SetUp \"1\"]")
            && text.contains("[FEN \"4k3/8/8/8/8/8/4P3/4K3 b - - 3 40\"]")
    );
}

#[test]
fn utf8_bom_and_crlf() {
    let pgn = "\u{feff}[Event \"Ü\"]\r\n\r\n{Комментарий\r\nв две строки} 1. d4 *\r\n";
    let p = parse_game(pgn.as_bytes(), TextEncoding::Utf8);
    assert_eq!(p.game.tag("Event"), Some("Ü"));
    assert_eq!(
        p.game.node(p.game.root()).expect("r").comments_after(),
        &["Комментарий\nв две строки".to_owned()]
    );
}

#[test]
fn comment_paragraphs_survive_writing() {
    // A book comment with paragraphs: lines are reflowed, blank lines kept.
    let p = parse_ok(
        "[Event \"?\"]\n\n1. e4 {First paragraph\nstill the first.\n\nSecond paragraph.\n\n\nThird.} e5 *\n",
    );
    let out = assert_roundtrip(&p);
    let again = parse_ok(&out);
    let e4 = again.game.mainline()[0];
    let text = chess_core::comment_display_text(again.game.node(e4).unwrap().comments_after());
    assert_eq!(
        text,
        "First paragraph still the first.\n\nSecond paragraph.\n\nThird."
    );
}
