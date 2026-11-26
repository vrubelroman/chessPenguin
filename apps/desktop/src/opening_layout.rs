//! What the three opening-training screens share (docs/design/opening-trainer-v2-spec.md
//! §1): the frame of the game screen, the card, the board's PGN marks and the texts
//! about moves, sides and dates.

use chess_core::shakmaty::{Color, Square, uci::UciMove};
use chess_core::{Annotation, CommentPart, Game, MoveKind, NodeId, parse_comment};
use egui::{Color32, RichText, Ui, vec2};
use trainer::repertoire::{LearningSide, RepertoireId, TrainingMode};

use crate::board::{Arrow, Badge};
use crate::i18n::{tr, trf};
use crate::settings::Settings;

/// The panel's least width (the game's reference tab, `REF_MIN`).
const SIDE_LEAST: f32 = 320.0;
/// Under a stacked board: the panel's least height (`STACK_BOTTOM_MIN`).
const STACK_BOTTOM_MIN: f32 = 300.0;
const BOARD_MIN: f32 = 220.0;
/// Space between the blocks of the panel.
pub(crate) const GAP: f32 = 6.0;
/// The new moves of one session and of "10 more new".
pub(crate) const NEW_PER_SESSION: usize = 10;

/// Header, side panel and board in the places of the game screen. The same panel id on
/// every screen of the section: switching screens never moves the board.
pub(crate) fn screen<T: ?Sized>(
    ui: &mut Ui,
    this: &mut T,
    header: impl FnOnce(&mut T, &mut Ui),
    side: impl FnOnce(&mut T, &mut Ui),
    board: impl FnOnce(&mut T, &mut Ui),
) {
    egui::Panel::top("opening_header").show(ui, |ui| header(this, ui));
    let width = ui.available_width();
    let height = ui.available_height();
    let board_side = (height - 16.0).min(width - SIDE_LEAST - 24.0);
    let board_stack = (width - 16.0).min(height - STACK_BOTTOM_MIN - 16.0);
    let stacked = board_stack >= 1.25 * board_side;
    let panel = if stacked {
        let most = (height - BOARD_MIN - 16.0).max(STACK_BOTTOM_MIN);
        let rest = (height - board_stack - 16.0).clamp(STACK_BOTTOM_MIN, most);
        egui::Panel::bottom("opening_side_stacked")
            .resizable(true)
            .default_size(rest)
            .size_range(STACK_BOTTOM_MIN..=most)
            .show(ui, |ui| side(this, ui))
    } else {
        let board = height - 16.0;
        let most = (width - BOARD_MIN - 24.0).max(SIDE_LEAST);
        let room = (width - board - 24.0).clamp(SIDE_LEAST, most);
        egui::Panel::right("opening_side")
            .resizable(true)
            .default_size((width * 0.4).clamp(260.0, 460.0).min(room))
            .size_range(SIDE_LEAST..=most)
            .show(ui, |ui| side(this, ui))
    };
    crate::widgets::check_panel("opening_side", &panel.response);
    egui::CentralPanel::default().show(ui, |ui| {
        let area = ui.available_rect_before_wrap();
        let side = area.width().min(area.height()).max(1.0).floor();
        let left = area.left() + ((area.width() - side) / 2.0).max(0.0).floor();
        let top = area.top() + ((area.height() - side) / 2.0).max(0.0).floor();
        let rect = egui::Rect::from_min_size(egui::pos2(left, top), vec2(side, side));
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| board(this, ui));
    });
}

/// The frame of a state card (the player's card of a game against the computer).
pub(crate) fn card(ui: &Ui, active: bool, alert: bool) -> egui::Frame {
    let mut frame = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(10, 6))
        .corner_radius(4)
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ));
    if active {
        frame = frame.fill(ui.visuals().selection.bg_fill.gamma_multiply(0.35));
    }
    if alert {
        frame = frame.stroke(egui::Stroke::new(1.5, ui.visuals().error_fg_color));
    }
    frame
}

/// The content size of a [`card`] filling a block of `height`: the frame's margins and
/// its stroke stay inside the block.
pub(crate) fn card_inner(ui: &Ui, height: f32) -> egui::Vec2 {
    vec2(ui.available_width() - 22.0, height - 14.0).max(egui::Vec2::ZERO)
}

/// A block of a fixed height: whatever it holds, what follows never moves.
pub(crate) fn fixed_block<R>(ui: &mut Ui, height: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let width = ui.available_width().max(1.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), egui::Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    add(&mut child)
}

/// One line of text cut with "…", the whole text on hover.
pub(crate) fn line(ui: &mut Ui, text: impl Into<RichText>) -> egui::Response {
    let text = text.into();
    let hover = text.text().to_owned();
    let response = ui.add(egui::Label::new(text).truncate());
    if hover.is_empty() {
        response
    } else {
        response.on_hover_text(hover)
    }
}

/// `text` cut on the left by whole words ("… 4. Ba4 Nf6") to fit `width` in `font`:
/// the last moves of a path matter most.
pub(crate) fn cut_left(ui: &Ui, text: &str, font: &egui::FontId, width: f32) -> String {
    let fits = |text: &str| {
        ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
        }) <= width
    };
    if fits(text) {
        return text.to_owned();
    }
    let mut rest = text;
    while let Some((_, tail)) = rest.split_once(' ') {
        rest = tail;
        let cut = format!("… {rest}");
        if fits(&cut) {
            return cut;
        }
    }
    format!("… {rest}")
}

/// One line cut on the left ([`cut_left`]), the whole text on hover.
pub(crate) fn line_left(ui: &mut Ui, text: &str, strong: bool) -> egui::Response {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let shown = cut_left(ui, text, &font, ui.available_width());
    let mut rich = RichText::new(shown);
    if strong {
        rich = rich.strong();
    }
    let response = ui.add(egui::Label::new(rich).truncate());
    if text.is_empty() {
        response
    } else {
        response.on_hover_text(text)
    }
}

/// The trained colour of a repertoire, kept in the settings since 2.2.0 (an old "both
/// colours" repertoire opens for White).
pub(crate) fn side_of(settings: &Settings, id: &RepertoireId) -> LearningSide {
    match settings.repertoire_modes.get(id.as_str()) {
        Some((TrainingMode::Black, _)) => LearningSide::Black,
        _ => LearningSide::White,
    }
}

pub(crate) fn set_side(settings: &mut Settings, id: &RepertoireId, side: LearningSide) {
    let mode = match side {
        LearningSide::White => TrainingMode::White,
        LearningSide::Black => TrainingMode::Black,
    };
    settings.repertoire_modes.insert(
        id.to_string(),
        (mode, trainer::repertoire::QueueMode::SpacedRepetition),
    );
}

pub(crate) fn king(side: LearningSide) -> &'static str {
    match side {
        LearningSide::White => "♔",
        LearningSide::Black => "♚",
    }
}

pub(crate) fn you_play(side: LearningSide) -> &'static str {
    tr(match side {
        LearningSide::White => "You play White",
        LearningSide::Black => "You play Black",
    })
}

pub(crate) fn orientation(side: LearningSide) -> Color {
    side.color()
}

/// "4. Ba4" or "4… Nf6" in the notation of the application.
pub(crate) fn move_text(game: &Game, node: NodeId) -> Option<String> {
    let (number, color) = game.move_number(node)?;
    let san = crate::san::show(&game.game_move(node)?.san).into_owned();
    Some(match color {
        Color::White => format!("{number}. {san}"),
        Color::Black => format!("{number}… {san}"),
    })
}

/// The symbol of a move's assessment (`!`, `?!` …) and its colour for text.
pub(crate) fn assessment(game: &Game, node: NodeId, dark: bool) -> Option<(&'static str, Color32)> {
    let nag = game
        .node(node)?
        .nags()
        .iter()
        .copied()
        .find(|nag| nag.is_move_assessment())?;
    Some((
        nag.symbol()?,
        crate::notation::assessment_text_color(nag, dark)?,
    ))
}

/// The node reached from the root by these UCI moves.
pub(crate) fn node_at(game: &Game, path: &[String]) -> Option<NodeId> {
    let mut at = NodeId::ROOT;
    for uci in path {
        let mv = uci
            .parse::<UciMove>()
            .ok()?
            .to_move(game.position(at)?)
            .ok()?;
        at = game.find_child(at, mv)?;
    }
    Some(at)
}

/// The UCI moves from the root to a node.
pub(crate) fn uci_path(game: &Game, node: NodeId) -> Vec<String> {
    game.path(node)
        .into_iter()
        .skip(1)
        .filter_map(|id| game.game_move(id).and_then(|mv| mv.uci()))
        .collect()
}

/// "C84 · Ruy Lopez: Closed": the most precise named position on the way to `node`.
pub(crate) fn opening_text(game: &Game, node: NodeId) -> String {
    let keys: Vec<u64> = game
        .path(node)
        .into_iter()
        .filter_map(|id| game.position(id))
        .map(chess_openings::key)
        .collect();
    match chess_openings::deepest(&keys) {
        Some((_, id)) => {
            let opening = chess_openings::get(id);
            format!("{} · {}", opening.eco, opening.name)
        }
        None => String::new(),
    }
}

/// The start a repertoire's lines share: the last move before the first branching.
pub(crate) fn common_start(game: &Game) -> NodeId {
    let mut at = NodeId::ROOT;
    while let [only] = game.children(at) {
        at = *only;
    }
    at
}

pub(crate) fn last_move(game: &Game, node: NodeId) -> Option<(Square, Square)> {
    match game.game_move(node)?.kind {
        MoveKind::Normal(mv) => Some((
            mv.from().unwrap_or_else(|| mv.to()),
            crate::board::ui_target(&mv),
        )),
        MoveKind::Null => None,
    }
}

/// The lines of a repertoire: the paths of its last moves, without switched-off
/// branches, in PGN order.
pub(crate) fn lines(game: &Game, disabled: impl Fn(&[String]) -> bool) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![(NodeId::ROOT, Vec::new())];
    while let Some((node, path)) = stack.pop() {
        let children: Vec<_> = game
            .children(node)
            .iter()
            .filter_map(|child| {
                let uci = game.game_move(*child)?.uci()?;
                let mut next = path.clone();
                next.push(uci);
                (!disabled(&next)).then_some((*child, next))
            })
            .collect();
        if children.is_empty() {
            if !path.is_empty() {
                out.push(path);
            }
        } else {
            stack.extend(children.into_iter().rev());
        }
    }
    out
}

/// The end of the local day: "today" in the counts includes everything due before it.
pub(crate) fn end_of_today() -> i64 {
    use chrono::{Local, TimeZone};
    let now = Local::now();
    let tomorrow = now.date_naive().succ_opt().unwrap_or(now.date_naive());
    tomorrow
        .and_hms_opt(0, 0, 0)
        .and_then(|midnight| Local.from_local_datetime(&midnight).earliest())
        .map_or(now.timestamp() + 24 * 60 * 60, |midnight| {
            midnight.timestamp() - 1
        })
}

pub(crate) fn now() -> i64 {
    chrono::Local::now().timestamp()
}

/// "today at 18:40", "tomorrow", "in 3 days", then a date as in the library.
pub(crate) fn when(at: i64, now: i64) -> String {
    use chrono::{Local, TimeZone};
    let (Some(at), Some(now)) = (
        Local.timestamp_opt(at, 0).single(),
        Local.timestamp_opt(now, 0).single(),
    ) else {
        return String::new();
    };
    let days = (at.date_naive() - now.date_naive()).num_days();
    match days {
        ..=0 => trf("today at {time}", &[("time", &at.format("%H:%M"))]),
        1 => tr("tomorrow").to_owned(),
        2..=6 => trf("in {days} days", &[("days", &days)]),
        _ => at.format("%d.%m.%Y").to_string(),
    }
}

#[derive(Default)]
pub(crate) struct BoardAnnotations {
    pub(crate) arrows: Vec<Arrow>,
    pub(crate) circles: Vec<(Square, Color32)>,
    pub(crate) badge: Option<Badge>,
}

/// The marks of a PGN move on the board: its assessment badge, `[%cal]` arrows and
/// `[%csl]` squares.
pub(crate) fn pgn_annotations_at(game: &Game, at: NodeId) -> BoardAnnotations {
    let mut out = BoardAnnotations::default();
    let Some(node) = game.node(at) else {
        return out;
    };
    let square = node.game_move().and_then(|game_move| match game_move.kind {
        MoveKind::Normal(mv) => Some(crate::board::ui_target(&mv)),
        MoveKind::Null => None,
    });
    if let Some(square) = square {
        out.badge = node
            .nags()
            .iter()
            .copied()
            .find(|nag| nag.is_move_assessment())
            .and_then(|nag| {
                Some(Badge {
                    square,
                    text: nag.symbol()?,
                    color: crate::notation::assessment_color(nag)?,
                })
            });
    }
    for comment in node.comments_after() {
        for part in parse_comment(comment) {
            match part {
                CommentPart::Annotation(Annotation::Arrows(list)) => {
                    out.arrows
                        .extend(list.into_iter().map(|(color, from, to)| Arrow {
                            from,
                            to,
                            color: crate::game_view::shape_color(color),
                            weight: 0.9,
                        }));
                }
                CommentPart::Annotation(Annotation::Squares(list)) => {
                    out.circles.extend(
                        list.into_iter()
                            .map(|(color, square)| (square, crate::game_view::shape_color(color))),
                    );
                }
                CommentPart::Text(_) | CommentPart::Annotation(_) => {}
            }
        }
    }
    out
}

/// The position after a node, for a board that only shows it.
pub(crate) fn position_of(game: &Game, node: NodeId) -> chess_core::shakmaty::Chess {
    game.position(node)
        .or_else(|| game.position(NodeId::ROOT))
        .cloned()
        .unwrap_or_default()
}

/// A board that only shows a position.
pub(crate) fn still_board(
    ui: &mut Ui,
    state: &mut crate::board::BoardState,
    settings: &Settings,
    pos: &chess_core::shakmaty::Chess,
    orientation: Color,
    last_move: Option<(Square, Square)>,
    annotations: &BoardAnnotations,
) {
    crate::board::BoardView {
        pos,
        orientation,
        last_move,
        arrows: &annotations.arrows,
        circles: &annotations.circles,
        badge: annotations.badge,
        interactive: false,
        show_coords: settings.show_coords,
        theme: settings.board_theme.theme(),
        pieces: settings.piece_set,
        animation: settings.animation.seconds(),
        premove: None,
        auto_queen: false,
        alert: None,
    }
    .show(ui, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pgn_node_badges_keep_the_existing_palette_and_special_move_destinations() {
        for (nag, symbol) in [
            (1, "!"),
            (2, "?"),
            (3, "!!"),
            (4, "??"),
            (5, "!?"),
            (6, "?!"),
        ] {
            let game = chess_core::parse_str(&format!("1. e4 ${nag} *")).game;
            let node = game.main_child(NodeId::ROOT).unwrap();
            let badge = pgn_annotations_at(&game, node).badge.unwrap();
            assert_eq!(badge.text, symbol);
            assert_eq!(badge.square, Square::E4);
            assert_eq!(
                badge.color,
                crate::notation::assessment_color(chess_core::Nag(nag)).unwrap()
            );
        }
        for (fen, san, expected) in [
            ("4k3/8/8/8/8/8/8/4K2R w K - 0 1", "O-O!!", Square::G1),
            ("4k3/P7/8/8/8/8/8/4K3 w - - 0 1", "a8=Q+!", Square::A8),
        ] {
            let game =
                chess_core::parse_str(&format!("[SetUp \"1\"]\n[FEN \"{fen}\"]\n\n1. {san} *"))
                    .game;
            let node = game.main_child(NodeId::ROOT).unwrap();
            assert_eq!(
                pgn_annotations_at(&game, node).badge.unwrap().square,
                expected
            );
        }
        let game = chess_core::parse_str("1. e4 $14 *").game;
        let node = game.main_child(NodeId::ROOT).unwrap();
        assert!(
            pgn_annotations_at(&game, node).badge.is_none(),
            "position assessments are not move badges"
        );
    }

    #[test]
    fn pgn_shapes_of_a_move_become_arrows_and_circles() {
        let game = chess_core::parse_str("1. e4 {[%cal Re2e4,Ge7e5] [%csl Ye4]} *").game;
        let node = game.main_child(NodeId::ROOT).unwrap();
        let marks = pgn_annotations_at(&game, node);
        assert_eq!(marks.arrows.len(), 2);
        assert_eq!(
            marks.arrows[0].color,
            crate::game_view::shape_color(chess_core::ShapeColor::Red)
        );
        assert_eq!(
            marks.circles,
            [(
                Square::E4,
                crate::game_view::shape_color(chess_core::ShapeColor::Yellow)
            )]
        );
        assert!(pgn_annotations_at(&game, NodeId::ROOT).badge.is_none());
    }

    #[test]
    fn common_start_stops_before_the_first_branching() {
        let game = chess_core::parse_str("1. e4 e5 2. Nf3 (2. Bc4) Nc6 *").game;
        let start = common_start(&game);
        assert_eq!(uci_path(&game, start), ["e2e4", "e7e5"]);
        let game = chess_core::parse_str("1. e4 (1. d4) *").game;
        assert_eq!(common_start(&game), NodeId::ROOT);
        let game = chess_core::parse_str("*").game;
        assert_eq!(common_start(&game), NodeId::ROOT);
    }

    #[test]
    fn lines_are_the_last_moves_in_pgn_order_without_switched_off_branches() {
        let game = chess_core::parse_str("1. e4 e5 (1... c5 2. Nf3) 2. Nf3 (2. Bc4) *").game;
        let all = lines(&game, |_| false);
        assert_eq!(
            all,
            [
                vec!["e2e4", "e7e5", "g1f3"],
                vec!["e2e4", "e7e5", "f1c4"],
                vec!["e2e4", "c7c5", "g1f3"],
            ]
            .map(|line| line.into_iter().map(str::to_owned).collect::<Vec<_>>())
        );
        let without_c5 = lines(&game, |path| {
            path.starts_with(&["e2e4".into(), "c7c5".into()])
        });
        assert_eq!(without_c5.len(), 2);
        let node = node_at(&game, &all[1]).unwrap();
        assert_eq!(uci_path(&game, node), all[1]);
        assert_eq!(move_text(&game, node).as_deref(), Some("2. Bc4"));
        let black = node_at(&game, &all[2][..2]).unwrap();
        assert_eq!(move_text(&game, black).as_deref(), Some("1… c5"));
    }

    #[test]
    fn dates_read_as_today_tomorrow_days_then_a_date() {
        let at = 1_800_000_000;
        assert!(when(at + 60, at).contains(':'));
        assert_eq!(when(at + 24 * 3600, at), tr("tomorrow"));
        assert!(when(at + 3 * 24 * 3600, at).contains('3'));
        assert_eq!(when(at + 30 * 24 * 3600, at).len(), 10);
        assert!(end_of_today() >= now());
        assert!(end_of_today() - now() < 24 * 3600 + 3600);
    }
}
