//! The card of a game put aside (docs/design/desktop-adjourned.md): on the library's
//! shelf, as a strip under a folded shelf, and in the question about a new game.

use chess_core::shakmaty::{Chess, Color, Position, Square};
use egui::{RichText, Sense, Ui, vec2};

use crate::i18n::{tr, trf};
use crate::player_games::PutAside;
use crate::settings::Settings;

/// What the card shows, read once from the game put aside.
pub struct Card {
    pos: Chess,
    last: Option<(Square, Square)>,
    user: Color,
    /// «Game against Maia 1550».
    pub title: String,
    /// «Alekhine · you play black · move 18».
    pub line2: String,
    /// «5+3 · you 4:12 · Maia 3:58», «no clock».
    pub line3: String,
    /// The lines of its tooltip under the board.
    tip: Vec<String>,
    /// Where × writes it (`None`: × deletes it).
    pub db: Option<String>,
}

/// What was pressed on a card.
#[derive(PartialEq)]
pub enum Pressed {
    Continue,
    Discard,
}

impl Card {
    pub fn new(aside: &PutAside) -> Self {
        let game = crate::play::parse_game(aside.pgn()).unwrap_or_default();
        let saved = aside.game();
        let user = if saved.white {
            Color::White
        } else {
            Color::Black
        };
        let end = game.line_end(chess_core::NodeId::ROOT);
        let pos = game.position(end).cloned().unwrap_or_default();
        let last = game.game_move(end).and_then(|m| match &m.kind {
            chess_core::MoveKind::Normal(m) => Some((m.from()?, m.to())),
            chess_core::MoveKind::Null => None,
        });
        let style = crate::play::style(&saved.style);
        let (rival, short) = match saved.rival {
            crate::play::Rival::Maia => (format!("Maia {}", saved.strength), "Maia".to_owned()),
            crate::play::Rival::Rodent => {
                let name = style.rodent.name();
                (format!("{name} {}", saved.strength), name.to_owned())
            }
        };
        let side = if saved.white {
            tr("you play white")
        } else {
            tr("you play black")
        };
        let n = pos.fullmoves().get();
        let mv = trf("move {n}", &[("n", &n)]);
        let mut line2 = Vec::new();
        if saved.rival == crate::play::Rival::Rodent && saved.style != crate::play::STYLES[0].id {
            line2.push(tr(style.title()).to_owned());
        }
        line2.push(side.to_owned());
        line2.push(mv);
        let line3 = match saved.clock {
            Some(tc) => {
                let left = crate::play::times_left(&game, saved.floor, aside.to_move_left());
                let base = std::time::Duration::from_secs(tc.base.into());
                let text = |c: Color| {
                    crate::play::clock_text(
                        left[if c == Color::White { 0 } else { 1 }].unwrap_or(base),
                    )
                };
                format!(
                    "{} · {} · {short} {}",
                    tc.label(),
                    trf("you {t}", &[("t", &text(user))]),
                    text(!user)
                )
            }
            None => tr("no clock").to_owned(),
        };
        let whose = if pos.turn() == user {
            trf("move {n}, yours", &[("n", &n)])
        } else {
            trf("move {n}, {rival} to move", &[("n", &n), ("rival", &short)])
        };
        let when = chrono::DateTime::<chrono::Local>::from(aside.when())
            .format("%d.%m, %H:%M")
            .to_string();
        let mut tip = vec![trf("Game against {rival}", &[("rival", &rival)])];
        if line2.len() > 2 {
            tip.push(format!("{} · {}", line2[0], side));
        } else {
            tip.push(side.to_owned());
        }
        tip.push(whose);
        tip.push(trf("Put aside {when}", &[("when", &when)]));
        Card {
            pos,
            last,
            user,
            title: trf("Game against {rival}", &[("rival", &rival)]),
            line2: line2.join(" · "),
            line3,
            tip,
            db: aside.target().map(crate::own_db::base_name),
        }
    }

    /// The board of the position, `size` square, at `at`.
    pub fn board(&self, ui: &mut Ui, rect: egui::Rect, settings: &Settings) {
        let view = crate::board::BoardView {
            pos: &self.pos,
            orientation: self.user,
            last_move: self.last,
            arrows: &[],
            circles: &[],
            badge: None,
            interactive: false,
            show_coords: false,
            theme: settings.board_theme.theme(),
            pieces: settings.piece_set,
            animation: 0.0,
            premove: None,
            auto_queen: false,
            alert: None,
        };
        let mut state = crate::board::BoardState::default();
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            view.show(ui, &mut state);
        });
    }

    /// The board and the lines under the pointer.
    fn tooltip(&self, ui: &mut Ui, settings: &Settings) {
        let (rect, _) = ui.allocate_exact_size(vec2(192.0, 192.0), Sense::hover());
        self.board(ui, rect, settings);
        ui.add_space(4.0);
        ui.label(RichText::new(&self.tip[0]).strong());
        for line in &self.tip[1..] {
            ui.label(line);
        }
        ui.label(RichText::new(tr("Click, or Ctrl+G, Enter")).weak());
    }

    fn discard_hint(&self) -> String {
        match &self.db {
            Some(db) => trf(
                "Don't finish it: the game is written to «{db}» unfinished",
                &[("db", db)],
            ),
            None => tr("Don't finish it: the game is deleted").to_owned(),
        }
    }

    /// The width of the full card: its lines, its button, within 360…460.
    pub fn width(&self, ui: &Ui) -> f32 {
        let w = |s: &str, size: f32| {
            ui.fonts_mut(|f| {
                f.layout_no_wrap(
                    s.to_owned(),
                    egui::FontId::proportional(size),
                    egui::Color32::WHITE,
                )
                .size()
                .x
            })
        };
        let lines = w(&self.title, 14.0)
            .max(w(&self.line2, 12.0))
            .max(w(&self.line3, 12.0));
        let button = w(tr("Continue"), 14.0) + 2.0 * ui.spacing().button_padding.x;
        (64.0 + 12.0 + lines + 12.0 + button + 8.0 + 20.0 + 4.0).clamp(360.0, 460.0)
    }

    /// The card on the shelf: 64 high, `width` wide.
    pub fn show(&self, ui: &mut Ui, width: f32, settings: &Settings) -> Option<Pressed> {
        self.frame(ui, vec2(width, 64.0), settings, |ui, rect, card| {
            let v = ui.visuals().clone();
            let button = tr("Continue");
            let bw = ui.fonts_mut(|f| {
                f.layout_no_wrap(
                    button.to_owned(),
                    egui::FontId::proportional(14.0),
                    v.text_color(),
                )
                .size()
                .x
            }) + 2.0 * ui.spacing().button_padding.x;
            let text_left = rect.left() + 64.0 + 12.0;
            let text_w = rect.right() - 4.0 - 20.0 - 8.0 - bw - 12.0 - text_left;
            let mut y = rect.top() + 7.0;
            for (line, size, color) in [
                (&card.title, 14.0, v.text_color()),
                (&card.line2, 12.0, v.weak_text_color()),
                (&card.line3, 12.0, v.weak_text_color()),
            ] {
                let galley = cut(ui, line, size, color, text_w);
                let h = galley.size().y;
                ui.painter().galley(egui::pos2(text_left, y), galley, color);
                y += h + 2.0;
            }
            let at = egui::Rect::from_min_size(
                egui::pos2(rect.right() - 4.0 - 20.0 - 8.0 - bw, rect.center().y - 14.0),
                vec2(bw, 28.0),
            );
            ui.put(at, primary_button(ui, button)).clicked()
        })
    }

    /// The strip under a folded shelf: 40 high, one line.
    pub fn strip(&self, ui: &mut Ui, settings: &Settings) -> Option<Pressed> {
        let width = ui.available_width();
        self.frame(ui, vec2(width, 40.0), settings, |ui, rect, card| {
            let v = ui.visuals().clone();
            let button = tr("Continue");
            let bw = ui.fonts_mut(|f| {
                f.layout_no_wrap(
                    button.to_owned(),
                    egui::FontId::proportional(14.0),
                    v.text_color(),
                )
                .size()
                .x
            }) + 2.0 * ui.spacing().button_padding.x;
            let text_left = rect.left() + 40.0 + 10.0;
            let text_w = rect.right() - 4.0 - 20.0 - 8.0 - bw - 10.0 - text_left;
            let line = format!("{} · {} · {}", card.title, card.line2, card.line3);
            let galley = cut(ui, &line, 13.0, v.text_color(), text_w);
            let y = rect.center().y - galley.size().y / 2.0;
            ui.painter()
                .galley(egui::pos2(text_left, y), galley, v.text_color());
            let at = egui::Rect::from_min_size(
                egui::pos2(rect.right() - 4.0 - 20.0 - 8.0 - bw, rect.center().y - 14.0),
                vec2(bw, 28.0),
            );
            ui.put(at, primary_button(ui, button)).clicked()
        })
    }

    /// The card's frame, board, × and click; `inside` draws the rest and says whether its
    /// button was pressed.
    fn frame(
        &self,
        ui: &mut Ui,
        size: egui::Vec2,
        settings: &Settings,
        inside: impl FnOnce(&mut Ui, egui::Rect, &Card) -> bool,
    ) -> Option<Pressed> {
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let v = ui.visuals();
        let fill = if ui.rect_contains_pointer(rect) {
            v.widgets.hovered.weak_bg_fill
        } else {
            v.faint_bg_color
        };
        let stroke = egui::Stroke::new(1.5, v.selection.bg_fill);
        ui.painter().rect_filled(rect, 6.0, fill);
        // Inside the rounded frame, clear of its outline.
        let side = rect.height() - 8.0;
        let board = egui::Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(side, side));
        self.board(ui, board, settings);
        ui.painter()
            .rect_stroke(rect, 6.0, stroke, egui::StrokeKind::Inside);
        // The whole card goes on with the game, its board too (over it: later); the
        // button and × are their own.
        let resp = ui
            .interact(rect, ui.id().with("aside_card"), Sense::click())
            .on_hover_ui(|ui| self.tooltip(ui, settings));
        let mut pressed = None;
        if inside(ui, rect, self) {
            pressed = Some(Pressed::Continue);
        }
        let x = egui::Rect::from_min_size(rect.right_top() + vec2(-26.0, 4.0), vec2(22.0, 20.0));
        if ui
            .put(x, |ui: &mut Ui| crate::widgets::small_close_button(ui))
            .on_hover_text(self.discard_hint())
            .clicked()
        {
            pressed = Some(Pressed::Discard);
        }
        if pressed.is_none() && resp.clicked() {
            pressed = Some(Pressed::Continue);
        }
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                true,
                format!(
                    "{}: {} · {} · {}. {}",
                    tr("Game put aside"),
                    self.title,
                    self.line2,
                    self.line3,
                    tr("Continue")
                ),
            )
        });
        pressed
    }

    /// The board and the three lines, for the question about a new game.
    pub fn summary(&self, ui: &mut Ui, settings: &Settings) {
        ui.horizontal_top(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(96.0, 96.0), Sense::hover());
            self.board(ui, rect, settings);
            ui.add_space(12.0 - ui.spacing().item_spacing.x);
            ui.vertical(|ui| {
                ui.label(RichText::new(&self.title).size(14.0).strong());
                ui.label(RichText::new(&self.line2).weak());
                ui.label(RichText::new(&self.line3).weak());
            });
        });
    }
}

/// A line cut with «…» to `width`.
fn cut(
    ui: &Ui,
    text: &str,
    size: f32,
    color: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(
        text.to_owned(),
        egui::FontId::proportional(size),
        color,
        width.max(10.0),
    );
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    ui.fonts_mut(|f| f.layout_job(job))
}

/// As `widgets::primary`, to be put at a place.
fn primary_button(ui: &Ui, label: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(label.to_owned()).strong()).fill(ui.visuals().selection.bg_fill)
}
