//! The user's own databases (docs/design/own-databases.md): the dialogs of creating a
//! PGN database, choosing one to add a game to, the game's data, a duplicate, deleting
//! a game, a copy in UTF-8, errors of writing. Each dialog draws itself and tells what
//! was chosen; the flows between them live in `app.rs`.

use std::path::{Path, PathBuf};

use chess_core::{Game, GameResult};
use egui::{Key, RichText, Ui};

use crate::i18n::{tr, trf};

/// Files bigger than this are rewritten with a progress window and, once a session,
/// the advice to keep one's own games in a smaller database (§6.4).
pub const BIG_REWRITE_BYTES: u64 = 50 << 20;

/// Why a database cannot take a game (§5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// Downloaded from Lichess and Chess.com: updating it would drop added games.
    Online,
    /// A ChessBase database or an archive: read only.
    ReadOnly,
    /// A sample database of the program: read only.
    Example,
    /// A PGN in another encoding: a UTF-8 copy can take games.
    Encoding(&'static str),
    /// The file cannot be written (permissions).
    NoPermission,
    /// The file is not on the disk any more.
    Missing,
}

impl Block {
    /// The explanation when the user tries anyway.
    pub fn long(&self) -> String {
        match self {
            Block::Online => tr("This database is downloaded from Lichess and Chess.com and updates itself: a game added by hand would be lost at the next update. Add the game to a database of your own.").to_owned(),
            Block::ReadOnly => tr("ChessBase databases and archives are only read. Add the game to a PGN database of your own.").to_owned(),
            Block::Encoding(enc) => trf("This file is in the {enc} encoding; games are written in UTF-8. Make a copy of it in UTF-8 and add games to the copy — the original stays as it is.", &[("enc", enc)]),
            Block::Example => tr("This is a sample database of the program; it is only read. Make a database of your own for your games.").to_owned(),
            Block::NoPermission => tr("The file cannot be written: no permission.").to_owned(),
            Block::Missing => tr("The file is not on the disk any more.").to_owned(),
        }
    }
}

/// The user's Documents folder (else the home folder).
pub fn documents_dir() -> PathBuf {
    let dirs = directories::UserDirs::new();
    dirs.as_ref()
        .and_then(|u| u.document_dir().map(Path::to_owned))
        .or_else(|| dirs.as_ref().map(|u| u.home_dir().to_owned()))
        .unwrap_or_else(std::env::temp_dir)
}

/// Where new databases go unless the user chose another folder.
pub fn default_base_dir(chosen: Option<&Path>) -> PathBuf {
    chosen
        .map(Path::to_owned)
        .unwrap_or_else(|| documents_dir().join("ChessPenguin"))
}

/// The name of a database as the shelf shows it: the file's stem.
pub fn base_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Characters a file name cannot have (on any of the three systems).
pub const FORBIDDEN: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

// --- New database ------------------------------------------------------------------------

/// What to do once the database is created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AfterCreate {
    /// Just put it on the shelf.
    Shelf,
    /// Add the current game to it (the chooser led here).
    AddGame,
    /// Add a copy of a record of a table to it.
    AddRecord { path: PathBuf, index: usize },
}

pub struct NewBaseDialog {
    pub name: String,
    pub dir: PathBuf,
    pub after: AfterCreate,
    /// Creating the file failed: said under the name, the dialog stays open.
    pub error: Option<String>,
    focus: bool,
}

pub enum NewBaseOutcome {
    Create(PathBuf),
    /// The file exists: put it on the shelf as it is.
    AddExisting(PathBuf),
    PickFolder,
    Cancel,
}

impl NewBaseDialog {
    pub fn new(dir: PathBuf, after: AfterCreate) -> Self {
        Self {
            name: String::new(),
            dir,
            after,
            error: None,
            focus: true,
        }
    }

    /// The name as typed, without a ".pgn" the user may have added.
    fn stem(&self) -> &str {
        let name = self.name.trim();
        match name.get(name.len().saturating_sub(4)..) {
            Some(ext) if ext.eq_ignore_ascii_case(".pgn") => name[..name.len() - 4].trim_end(),
            _ => name,
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.join(format!("{}.pgn", self.stem()))
    }

    /// What is wrong with the name, if anything; `Ok(true)` when a file with this name
    /// exists already.
    fn check(&self) -> Result<bool, String> {
        let name = self.stem();
        if name.is_empty() {
            return Err(String::new());
        }
        if name.contains(FORBIDDEN) || name.chars().all(|c| c == '.') {
            return Err(tr("A name cannot have / \\ : * ? \" < > |").to_owned());
        }
        Ok(self.path().exists())
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<NewBaseOutcome> {
        let mut out = None;
        egui::Modal::new(egui::Id::new("new_base")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(tr("New database"));
            ui.add_space(8.0);
            ui.label(tr("Title"));
            let check = self.check();
            let mut edit = egui::TextEdit::singleline(&mut self.name)
                .hint_text(tr("e.g. Petrov, My games, Sicilian"))
                .desired_width(f32::INFINITY);
            if matches!(&check, Err(e) if !e.is_empty()) {
                edit = edit.text_color(ui.visuals().error_fg_color);
            }
            let edit = ui.add(edit);
            if edit.changed() {
                self.error = None;
            }
            if std::mem::take(&mut self.focus) {
                edit.request_focus();
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(tr("File:")).weak());
                // The folder alone until a name is typed.
                let shown = if self.name.trim().is_empty() {
                    format!("{}{}", self.dir.display(), std::path::MAIN_SEPARATOR)
                } else {
                    self.path().display().to_string()
                };
                ui.label(RichText::new(shown).weak());
            });
            // On a line of its own: typing the name does not move it.
            if ui.small_button(tr("Change folder…")).clicked() {
                out = Some(NewBaseOutcome::PickFolder);
            }
            match &check {
                Err(e) if !e.is_empty() => {
                    ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
                }
                Ok(true) => {
                    ui.label(
                        RichText::new(tr("A file with this name is already there."))
                            .color(ui.visuals().warn_fg_color),
                    );
                }
                _ => {}
            }
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    match &check {
                        Ok(false) => {
                            if crate::widgets::primary(ui, tr("Create")).clicked() || enter {
                                out = Some(NewBaseOutcome::Create(self.path()));
                            }
                        }
                        Ok(true) => {
                            if crate::widgets::primary(ui, tr("Put it on the shelf")).clicked()
                                || enter
                            {
                                out = Some(NewBaseOutcome::AddExisting(self.path()));
                            }
                        }
                        // The same main button, only disabled: the row does not jump
                        // when the first letter is typed.
                        Err(_) => {
                            ui.add_enabled_ui(false, |ui| {
                                crate::widgets::primary(ui, tr("Create"))
                            });
                        }
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        out = Some(NewBaseOutcome::Cancel);
                    }
                });
            });
        });
        if out.is_none() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            out = Some(NewBaseOutcome::Cancel);
        }
        out
    }
}

// --- Add to database --------------------------------------------------------------------

/// A database on the shelf, as the chooser lists it.
pub struct ChooserRow {
    pub path: PathBuf,
    pub name: String,
    /// `None`: not read yet (its encoding is not known either).
    pub games: Option<usize>,
    pub block: Option<Block>,
}

impl ChooserRow {
    /// Read, and nothing blocks it: can be chosen.
    pub fn takes_games(&self) -> bool {
        self.games.is_some() && self.block.is_none()
    }
}

/// What the chooser is for: the game on the screen, or a record of a table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChooserFor {
    CurrentGame,
    Record { path: PathBuf, index: usize },
}

pub struct Chooser {
    pub rows: Vec<ChooserRow>,
    pub selected: Option<usize>,
    pub search: String,
    pub purpose: ChooserFor,
    /// The game's title, for the heading.
    pub title: String,
    focus: bool,
}

pub enum ChooserOutcome {
    Pick(PathBuf),
    NewBase,
    SaveAsFile,
    Cancel,
}

impl Chooser {
    pub fn new(
        rows: Vec<ChooserRow>,
        preselect: Option<&Path>,
        purpose: ChooserFor,
        title: String,
    ) -> Self {
        let selected = preselect
            .and_then(|p| rows.iter().position(|r| r.path == p && r.takes_games()))
            .or_else(|| rows.iter().position(ChooserRow::takes_games));
        Self {
            rows,
            selected,
            search: String::new(),
            purpose,
            title,
            focus: true,
        }
    }

    /// The shelf as it is now (databases get read while the dialog is open): the
    /// chosen row stays chosen by its path.
    pub fn refresh(&mut self, rows: Vec<ChooserRow>) {
        let chosen = self
            .selected
            .and_then(|i| self.rows.get(i))
            .map(|r| r.path.clone());
        self.rows = rows;
        self.selected = chosen
            .and_then(|p| {
                self.rows
                    .iter()
                    .position(|r| r.path == p && r.takes_games())
            })
            .or_else(|| self.rows.iter().position(ChooserRow::takes_games));
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<ChooserOutcome> {
        let mut out = None;
        egui::Modal::new(egui::Id::new("add_to_base")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(tr("Add to database"));
            if !self.title.is_empty() {
                ui.label(RichText::new(&self.title).weak());
            }
            ui.add_space(6.0);
            // The field that narrows the list, for a long list only (of the databases
            // offered, not of the whole shelf).
            let many = self.rows.iter().filter(|r| r.block.is_none()).count() > 8;
            if many {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text(tr("🔍 Find a database"))
                        .desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut self.focus) {
                    edit.request_focus();
                }
            }
            let needle = self.search.trim().to_lowercase();
            let shown: Vec<usize> = (0..self.rows.len())
                .filter(|&i| {
                    needle.is_empty() || self.rows[i].name.to_lowercase().contains(&needle)
                })
                .collect();
            // Only the databases that take games, in shelf order; those still being read
            // after them (they may turn out to take games). Online, ChessBase, legacy
            // encodings, read-only files are not offered at all.
            let order: Vec<usize> = shown
                .iter()
                .copied()
                .filter(|&i| self.rows[i].takes_games())
                .chain(shown.iter().copied().filter(|&i| {
                    let r = &self.rows[i];
                    r.block.is_none() && r.games.is_none()
                }))
                .collect();
            let (up, down, enter) = ui.input(|i| {
                (
                    i.key_pressed(Key::ArrowUp),
                    i.key_pressed(Key::ArrowDown),
                    i.key_pressed(Key::Enter),
                )
            });
            if up || down {
                let writable: Vec<usize> = order
                    .iter()
                    .copied()
                    .filter(|&i| self.rows[i].takes_games())
                    .collect();
                let pos = self
                    .selected
                    .and_then(|s| writable.iter().position(|&i| i == s));
                let next = match (pos, down) {
                    (None, _) => 0,
                    (Some(p), true) => (p + 1).min(writable.len().saturating_sub(1)),
                    (Some(p), false) => p.saturating_sub(1),
                };
                self.selected = writable.get(next).copied();
            }
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    if order.is_empty() {
                        ui.label(
                            RichText::new(tr("No databases of your own yet: create one below."))
                                .weak(),
                        );
                    }
                    // Rows the whole width, their text from the left.
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                        for i in order {
                            let row = &self.rows[i];
                            let selected = self.selected == Some(i);
                            let text = match row.games {
                                Some(n) => format!("{} · {}", row.name, crate::i18n::games(n)),
                                None => format!("{} · {}", row.name, tr("reading…")),
                            };
                            let folder = row
                                .path
                                .parent()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default();
                            match (&row.block, row.games) {
                                (None, Some(_)) => {
                                    let resp = ui
                                        .add(egui::Button::selectable(selected, text))
                                        .on_hover_text(&folder);
                                    if resp.clicked() {
                                        self.selected = Some(i);
                                    }
                                    if resp.double_clicked() {
                                        out = Some(ChooserOutcome::Pick(row.path.clone()));
                                    }
                                }
                                (None, None) => {
                                    ui.label(RichText::new(text).weak());
                                }
                                (Some(_), _) => {}
                            }
                        }
                    });
                });
            ui.separator();
            // Nothing to choose: the new database is the main way, not «Add».
            let none = !self.rows.iter().any(|r| r.block.is_none());
            let new_base = if none {
                crate::widgets::primary(ui, tr("✚ New database…"))
            } else {
                ui.button(tr("✚ New database…"))
            };
            if new_base.clicked() || (none && ui.input(|i| i.key_pressed(Key::Enter))) {
                out = Some(ChooserOutcome::NewBase);
            }
            if self.purpose == ChooserFor::CurrentGame
                && ui
                    .button(tr("Save as a file…"))
                    .on_hover_text(tr(
                        "A separate PGN file with this game only; it is not put on the shelf",
                    ))
                    .clicked()
            {
                out = Some(ChooserOutcome::SaveAsFile);
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let pick = self.selected.map(|i| self.rows[i].path.clone());
                if !none {
                    let add = ui
                        .add_enabled_ui(pick.is_some(), |ui| crate::widgets::primary(ui, tr("Add")))
                        .inner;
                    if (add.clicked() || enter)
                        && let Some(p) = pick
                    {
                        out = Some(ChooserOutcome::Pick(p));
                    }
                }
                if ui.button(tr("Cancel")).clicked() {
                    out = Some(ChooserOutcome::Cancel);
                }
            });
        });
        if out.is_none() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            out = Some(ChooserOutcome::Cancel);
        }
        out
    }
}

// --- Game data --------------------------------------------------------------------------

/// What the main button of the editor does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorMode {
    /// Changes the game (Ctrl+S saves them).
    Done,
    /// Then the game is added to this database.
    Add(PathBuf),
}

pub struct DataEditor {
    pub white: String,
    pub white_elo: String,
    pub black: String,
    pub black_elo: String,
    pub event: String,
    pub site: String,
    pub date: String,
    pub round: String,
    pub result: GameResult,
    pub annotator: String,
    /// The tags the editor does not show (kept as they are).
    pub others: Vec<String>,
    pub mode: EditorMode,
    /// The fields as the editor opened (nothing changed: nothing written).
    initial: String,
    focus: bool,
}

pub enum EditorOutcome {
    Done,
    Cancel,
}

fn shown(value: Option<&str>) -> String {
    value
        .filter(|v| !v.is_empty() && !v.chars().all(|c| c == '?' || c == '.'))
        .unwrap_or("")
        .to_owned()
}

impl DataEditor {
    pub fn from_game(game: &Game, your_name: &str, mode: EditorMode) -> Self {
        let t = |n: &str| shown(game.tag(n));
        let known = [
            "Event",
            "Site",
            "Date",
            "Round",
            "White",
            "Black",
            "Result",
            "WhiteElo",
            "BlackElo",
            "Annotator",
            "FEN",
            "SetUp",
            "PlyCount",
        ];
        let others: Vec<String> = game
            .tags()
            .iter()
            .filter(|(k, _)| !known.contains(&k.as_str()))
            .map(|(k, _)| k.clone())
            .collect();
        let annotator = {
            let a = t("Annotator");
            if a.is_empty() && game.tag("Annotator").is_none() {
                your_name.to_owned()
            } else {
                a
            }
        };
        let mut editor = Self {
            white: t("White"),
            white_elo: t("WhiteElo"),
            black: t("Black"),
            black_elo: t("BlackElo"),
            event: t("Event"),
            site: t("Site"),
            date: t("Date"),
            round: t("Round"),
            result: game.result(),
            annotator,
            others,
            mode,
            initial: String::new(),
            focus: true,
        };
        editor.initial = editor.state();
        editor
    }

    fn state(&self) -> String {
        [
            &self.white,
            &self.white_elo,
            &self.black,
            &self.black_elo,
            &self.event,
            &self.site,
            &self.date,
            &self.round,
            &self.annotator,
        ]
        .iter()
        .map(|s| s.trim())
        .collect::<Vec<_>>()
        .join("\u{1}")
            + self.result.as_str()
    }

    /// Something was edited.
    pub fn changed(&self) -> bool {
        self.state() != self.initial
    }

    fn date_ok(&self) -> bool {
        chess_core::pgn::store::pgn_date(&self.date).is_some()
    }

    /// Writes the fields into the game (empty fields as PGN's "?"). Opened and closed
    /// without an edit, it writes nothing — unless the game is being added: then the
    /// annotator and the standard tags go in.
    pub fn apply(&self, game: &mut Game) {
        if !self.changed() && self.mode == EditorMode::Done {
            return;
        }
        let set = |game: &mut Game, name: &str, value: &str, unknown: &str| {
            let v = value.trim();
            if v.is_empty() {
                // A cleared field: the seven PGN tags say "?", an optional one goes away.
                if unknown.is_empty() {
                    game.remove_tag(name);
                } else if game.tag(name) != Some(unknown) {
                    game.set_tag(name, unknown);
                }
            } else if game.tag(name) != Some(v) {
                game.set_tag(name, v);
            }
        };
        set(game, "White", &self.white, "?");
        set(game, "Black", &self.black, "?");
        set(game, "Event", &self.event, "?");
        set(game, "Site", &self.site, "?");
        set(game, "Round", &self.round, "?");
        let date =
            chess_core::pgn::store::pgn_date(&self.date).unwrap_or_else(|| "????.??.??".to_owned());
        if game.tag("Date") != Some(date.as_str()) {
            game.set_tag("Date", &date);
        }
        set(game, "WhiteElo", &self.white_elo, "");
        set(game, "BlackElo", &self.black_elo, "");
        set(game, "Annotator", &self.annotator, "");
        if game.result() != self.result {
            game.set_result(self.result);
        }
        game.order_roster();
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<EditorOutcome> {
        let mut out = None;
        egui::Modal::new(egui::Id::new("game_data")).show(ctx, |ui| {
            ui.set_width(460.0);
            match &self.mode {
                EditorMode::Done => ui.heading(tr("Game data")),
                EditorMode::Add(p) => {
                    ui.heading(trf("Game data → «{name}»", &[("name", &base_name(p))]))
                }
            };
            ui.add_space(8.0);
            let label_w = 90.0;
            let field = |ui: &mut Ui, text: &mut String, hint: &str, width: f32| {
                ui.add(
                    egui::TextEdit::singleline(text)
                        .hint_text(hint)
                        .desired_width(width),
                )
            };
            let row = |ui: &mut Ui, label: &str, add: &mut dyn FnMut(&mut Ui)| {
                ui.horizontal(|ui| {
                    ui.add_sized([label_w, 20.0], egui::Label::new(label));
                    add(ui);
                });
            };
            let wide = 460.0 - label_w - 24.0;
            let mut first = None;
            let elo_w = 56.0;
            let elo_label = crate::game_view::text_width(ui, "Elo", egui::TextStyle::Body) + 16.0;
            row(ui, tr("White"), &mut |ui| {
                let r = field(
                    ui,
                    &mut self.white,
                    tr("Surname, Name"),
                    wide - elo_w - elo_label,
                );
                first = Some(r);
                ui.label("Elo");
                field(ui, &mut self.white_elo, "", elo_w);
            });
            row(ui, tr("Black"), &mut |ui| {
                field(
                    ui,
                    &mut self.black,
                    tr("Surname, Name"),
                    wide - elo_w - elo_label,
                );
                ui.label("Elo");
                field(ui, &mut self.black_elo, "", elo_w);
            });
            row(ui, tr("Event"), &mut |ui| {
                field(ui, &mut self.event, "", wide);
            });
            row(ui, tr("Site"), &mut |ui| {
                field(ui, &mut self.site, "", wide);
            });
            let date_ok = self.date_ok();
            row(ui, tr("Date"), &mut |ui| {
                let mut edit = egui::TextEdit::singleline(&mut self.date)
                    .hint_text(tr("YYYY.MM.DD"))
                    .desired_width(120.0);
                if !date_ok {
                    edit = edit.text_color(ui.visuals().error_fg_color);
                }
                ui.add(edit)
                    .on_hover_text(tr("2026.10.18, 18.10.2026, 2026.10, 2026 or empty"));
                ui.add_space(8.0);
                ui.label(tr("Round"));
                field(ui, &mut self.round, "", 60.0);
            });
            row(ui, tr("Result"), &mut |ui| {
                for (r, label) in [
                    (GameResult::WhiteWins, "1-0"),
                    (GameResult::Draw, "½-½"),
                    (GameResult::BlackWins, "0-1"),
                    (GameResult::Unknown, "*"),
                ] {
                    ui.selectable_value(&mut self.result, r, label);
                }
            });
            row(ui, tr("Annotator"), &mut |ui| {
                field(ui, &mut self.annotator, "", wide);
            });
            if !self.others.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(trf(
                        "Other tags stay as they are: {tags}",
                        &[("tags", &self.others.join(", "))],
                    ))
                    .weak(),
                );
            }
            if std::mem::take(&mut self.focus)
                && let Some(r) = first
            {
                // White's name selected: typing replaces it.
                r.request_focus();
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), r.id) {
                    let range = egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(self.white.chars().count()),
                    );
                    state.cursor.set_char_range(Some(range));
                    state.store(ui.ctx(), r.id);
                }
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = match &self.mode {
                    EditorMode::Done => tr("Done").to_owned(),
                    EditorMode::Add(p) => trf("Add to «{name}»", &[("name", &base_name(p))]),
                };
                let enter = ui.input(|i| i.key_pressed(Key::Enter));
                let ok = ui
                    .add_enabled_ui(date_ok, |ui| crate::widgets::primary(ui, &label))
                    .inner;
                if ok.clicked() || (enter && date_ok) {
                    out = Some(EditorOutcome::Done);
                }
                if ui.button(tr("Cancel")).clicked() {
                    out = Some(EditorOutcome::Cancel);
                }
            });
        });
        if out.is_none() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            out = Some(EditorOutcome::Cancel);
        }
        out
    }
}

// --- Small questions --------------------------------------------------------------------

/// "Yes / no / cancel" questions, each with its own heading, text and buttons.
pub struct Question {
    pub kind: QuestionKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionKind {
    /// The same game is in the target already (record `index`).
    Duplicate {
        target: PathBuf,
        index: usize,
        title: String,
    },
    /// Delete record `index` of `path`.
    Delete {
        path: PathBuf,
        index: usize,
        title: String,
    },
    /// Make a UTF-8 copy of `path` at `dest` and add the game to it.
    Utf8Copy {
        path: PathBuf,
        dest: PathBuf,
        encoding: &'static str,
    },
    /// Writing failed.
    WriteFailed {
        path: PathBuf,
        error: String,
        deleting: bool,
    },
    /// The file was changed by another program since it was read.
    ChangedOnDisk { path: PathBuf, deleting: bool },
    /// The file is big: replacing a record rewrites it all.
    BigBase { path: PathBuf, bytes: u64 },
}

/// What was chosen in a question, by its meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Replace the duplicate / delete / make the copy / retry / save anyway / reread.
    Primary,
    /// Add as a new game / add to another database.
    Secondary,
    Cancel,
}

impl Question {
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Answer> {
        let mut out = None;
        egui::Modal::new(egui::Id::new("own_db_question")).show(ctx, |ui| {
            ui.set_width(420.0);
            let (heading, text, primary, secondary, cancel): (String, String, Option<String>, Option<String>, &str) = match &self.kind {
                QuestionKind::Duplicate { target, index, title } => (
                    tr("This game is already there").to_owned(),
                    trf("«{name}» already has this game: No. {n}, {title} — the same players, date and moves.", &[("title", title), ("name", &base_name(target)), ("n", &(index + 1))]),
                    Some(trf("Replace No. {n}", &[("n", &(index + 1))])),
                    Some(tr("Add as new").to_owned()),
                    tr("Cancel"),
                ),
                QuestionKind::Delete { path, index, title } => (
                    tr("Delete the game from the database?").to_owned(),
                    trf("«{title}» (No. {n}) is removed from the file «{file}»; other databases are not touched. Undo is offered for a few seconds.", &[("title", title), ("n", &(index + 1)), ("file", &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())]),
                    Some(tr("Delete").to_owned()),
                    None,
                    tr("Cancel"),
                ),
                QuestionKind::Utf8Copy { path, dest, encoding } => (
                    tr("A copy in UTF-8").to_owned(),
                    trf("«{name}» is in the {enc} encoding, and games are written in UTF-8. A copy «{copy}» will be made next to it with all its games; the game is added to the copy, the original stays as it is.", &[("name", &base_name(path)), ("enc", encoding), ("copy", &dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())]),
                    Some(tr("Make the copy").to_owned()),
                    None,
                    tr("Cancel"),
                ),
                QuestionKind::WriteFailed { path, error, deleting } => (
                    if *deleting { tr("Could not delete").to_owned() } else { tr("Could not save").to_owned() },
                    format!("{}\n{}", path.display(), error),
                    Some(tr("Try again").to_owned()),
                    (!deleting).then(|| tr("Add to another database…").to_owned()),
                    tr("Close"),
                ),
                QuestionKind::ChangedOnDisk { path, deleting } => (
                    tr("The file changed on disk").to_owned(),
                    if *deleting {
                        trf("«{name}» was changed by another program since it was read. Reread it and look again.", &[("name", &base_name(path))])
                    } else {
                        trf("«{name}» was changed by another program since it was read. The game can be added to it as a new one; replacing a record in it would need a fresh read first.", &[("name", &base_name(path))])
                    },
                    if *deleting { Some(tr("Reread").to_owned()) } else { Some(tr("Add as new").to_owned()) },
                    None,
                    tr("Cancel"),
                ),
                QuestionKind::BigBase { path, bytes } => (
                    tr("A big database").to_owned(),
                    trf("«{name}» is {mb} MB: saving a game into it rewrites the whole file, which takes a while. Your own games are handier in a smaller database of their own.", &[("name", &base_name(path)), ("mb", &(bytes >> 20))]),
                    Some(tr("Save anyway").to_owned()),
                    Some(tr("Add to another database…").to_owned()),
                    tr("Cancel"),
                ),
            };
            ui.heading(heading);
            ui.add_space(6.0);
            ui.label(text);
            ui.add_space(10.0);
            // Replacing a record is never one Enter away: the duplicate question has no
            // main button (own-databases.md §5.5).
            let plain = matches!(self.kind, QuestionKind::Duplicate { .. });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enter = !plain && ui.input(|i| i.key_pressed(Key::Enter));
                if let Some(p) = primary {
                    let clicked = if plain {
                        ui.add(egui::Button::new(&p).min_size(egui::vec2(0.0, 30.0)))
                            .clicked()
                    } else {
                        crate::widgets::primary(ui, &p).clicked()
                    };
                    if clicked || enter {
                        out = Some(Answer::Primary);
                    }
                }
                if let Some(s) = secondary
                    && ui.button(s).clicked()
                {
                    out = Some(Answer::Secondary);
                }
                if ui.button(cancel).clicked() {
                    out = Some(Answer::Cancel);
                }
            });
        });
        if out.is_none() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            out = Some(Answer::Cancel);
        }
        out
    }
}

/// Shows the file in the system's file manager.
pub fn show_in_folder(path: &Path) {
    let path = path.to_path_buf();
    let _ = std::thread::Builder::new()
        .name("show-in-folder".into())
        .spawn(move || {
            #[cfg(target_os = "windows")]
            {
                let _ = std::process::Command::new("explorer")
                    .arg(format!("/select,{}", path.display()))
                    .spawn();
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open")
                    .arg("-R")
                    .arg(&path)
                    .spawn();
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                // The file manager selects the file when it can (the portal's ShowItems),
                // else its folder opens.
                let shown = std::process::Command::new("dbus-send")
                    .args([
                        "--session",
                        "--print-reply",
                        "--dest=org.freedesktop.FileManager1",
                        "/org/freedesktop/FileManager1",
                        "org.freedesktop.FileManager1.ShowItems",
                        &format!("array:string:file://{}", path.display()),
                        "string:",
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success());
                if !shown && let Some(dir) = path.parent() {
                    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_writes_tags_and_result() {
        let mut g = chess_core::parse_str("[Event \"?\"]\n[White \"?\"]\n\n1. e4 *\n").game;
        let mut e = DataEditor::from_game(&g, "Coach", EditorMode::Done);
        assert_eq!(e.annotator, "Coach");
        e.white = "Petrov, Ivan".into();
        e.white_elo = "1500".into();
        e.date = "18.10.2026".into();
        e.result = GameResult::WhiteWins;
        e.apply(&mut g);
        assert_eq!(g.tag("White"), Some("Petrov, Ivan"));
        assert_eq!(g.tag("WhiteElo"), Some("1500"));
        assert_eq!(g.tag("Date"), Some("2026.10.18"));
        assert_eq!(g.tag("Black"), Some("?"));
        assert_eq!(g.tag("Annotator"), Some("Coach"));
        assert_eq!(g.result(), GameResult::WhiteWins);
        // The seven standard tags first, in their order; Result among them.
        let names: Vec<&str> = g.tags().iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            &names[..7],
            &["Event", "Site", "Date", "Round", "White", "Black", "Result"]
        );
        assert_eq!(g.tag("Result"), Some("1-0"));
        assert!(g.is_modified());
        // Nothing changed: the game is not marked modified again.
        g.mark_saved();
        let e2 = DataEditor::from_game(&g, "Coach", EditorMode::Done);
        e2.apply(&mut g);
        assert!(!g.is_modified());
    }
}
