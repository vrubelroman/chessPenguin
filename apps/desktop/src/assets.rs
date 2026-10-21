//! Bundled piece sets, board themes and sounds (see THIRD_PARTY_NOTICES.md).

use crate::i18n::tr;
use egui::{Color32, Rect};
use serde::{Deserialize, Serialize};
use shakmaty::{Color, Piece, Role};

macro_rules! piece_set {
    ($name:literal) => {
        [
            include_bytes!(concat!("../assets/pieces/", $name, "/wK.svg")).as_slice(),
            include_bytes!(concat!("../assets/pieces/", $name, "/wQ.svg")),
            include_bytes!(concat!("../assets/pieces/", $name, "/wR.svg")),
            include_bytes!(concat!("../assets/pieces/", $name, "/wB.svg")),
            include_bytes!(concat!("../assets/pieces/", $name, "/wN.svg")),
            include_bytes!(concat!("../assets/pieces/", $name, "/wP.svg")),
            include_bytes!(concat!("../assets/pieces/", $name, "/bK.svg")),
            include_bytes!(concat!("../assets/pieces/", $name, "/bQ.svg")),
            include_bytes!(concat!("../assets/pieces/", $name, "/bR.svg")),
            include_bytes!(concat!("../assets/pieces/", $name, "/bB.svg")),
            include_bytes!(concat!("../assets/pieces/", $name, "/bN.svg")),
            include_bytes!(concat!("../assets/pieces/", $name, "/bP.svg")),
        ]
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PieceSet {
    #[default]
    Cburnett,
    Merida,
    Mpchess,
    Chessnut,
    Fantasy,
    Celtic,
    Spatial,
    Rhosgfx,
    Shapes,
    KiwenSuwi,
    Firi,
    Pixel,
    Letter,
    Pirouetti,
    Governor,
    Kosal,
    Metaltops,
}

impl PieceSet {
    pub const ALL: [PieceSet; 17] = [
        PieceSet::Cburnett,
        PieceSet::Merida,
        PieceSet::Mpchess,
        PieceSet::Chessnut,
        PieceSet::Fantasy,
        PieceSet::Celtic,
        PieceSet::Spatial,
        PieceSet::Rhosgfx,
        PieceSet::Shapes,
        PieceSet::KiwenSuwi,
        PieceSet::Firi,
        PieceSet::Pixel,
        PieceSet::Letter,
        PieceSet::Pirouetti,
        PieceSet::Governor,
        PieceSet::Kosal,
        PieceSet::Metaltops,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PieceSet::Cburnett => "cburnett",
            PieceSet::Merida => "merida",
            PieceSet::Mpchess => "mpchess",
            PieceSet::Chessnut => "chessnut",
            PieceSet::Fantasy => "fantasy",
            PieceSet::Celtic => "celtic",
            PieceSet::Spatial => "spatial",
            PieceSet::Rhosgfx => "rhosgfx",
            PieceSet::Shapes => "shapes",
            PieceSet::KiwenSuwi => "kiwen-suwi",
            PieceSet::Firi => "firi",
            PieceSet::Pixel => "pixel",
            PieceSet::Letter => "letter",
            PieceSet::Pirouetti => "pirouetti",
            PieceSet::Governor => "governor",
            PieceSet::Kosal => "kosal",
            PieceSet::Metaltops => "metaltops",
        }
    }

    fn files(self) -> [&'static [u8]; 12] {
        match self {
            PieceSet::Cburnett => piece_set!("cburnett"),
            PieceSet::Merida => piece_set!("merida"),
            PieceSet::Mpchess => piece_set!("mpchess"),
            PieceSet::Chessnut => piece_set!("chessnut"),
            PieceSet::Fantasy => piece_set!("fantasy"),
            PieceSet::Celtic => piece_set!("celtic"),
            PieceSet::Spatial => piece_set!("spatial"),
            PieceSet::Rhosgfx => piece_set!("rhosgfx"),
            PieceSet::Shapes => piece_set!("shapes"),
            PieceSet::KiwenSuwi => piece_set!("kiwen-suwi"),
            PieceSet::Firi => piece_set!("firi"),
            PieceSet::Pixel => piece_set!("pixel"),
            PieceSet::Letter => piece_set!("letter"),
            PieceSet::Pirouetti => piece_set!("pirouetti"),
            PieceSet::Governor => piece_set!("governor"),
            PieceSet::Kosal => piece_set!("kosal"),
            PieceSet::Metaltops => piece_set!("metaltops"),
        }
    }

    fn index(piece: Piece) -> usize {
        let r = match piece.role {
            Role::King => 0,
            Role::Queen => 1,
            Role::Rook => 2,
            Role::Bishop => 3,
            Role::Knight => 4,
            Role::Pawn => 5,
        };
        if piece.color == Color::White {
            r
        } else {
            r + 6
        }
    }

    fn uri(self, piece: Piece) -> String {
        format!("bytes://pieces/{}/{}.svg", self.name(), Self::index(piece))
    }

    pub fn paint(self, ui: &egui::Ui, piece: Piece, rect: Rect) {
        egui::Image::new(self.uri(piece)).paint_at(ui, rect);
    }

    pub fn paint_tinted(self, ui: &egui::Ui, piece: Piece, rect: Rect, tint: egui::Color32) {
        egui::Image::new(self.uri(piece))
            .tint(tint)
            .paint_at(ui, rect);
    }
}

/// Registers all bundled SVGs with egui's byte loader.
pub fn install(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
    for set in PieceSet::ALL {
        for (i, bytes) in set.files().into_iter().enumerate() {
            ctx.include_bytes(format!("bytes://pieces/{}/{i}.svg", set.name()), bytes);
        }
    }
    for (name, bytes) in TEXTURES {
        ctx.include_bytes(format!("bytes://boards/{name}"), *bytes);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BoardThemeId {
    #[default]
    Brown,
    Blue,
    Green,
    Tournament,
    Sand,
    Coral,
    Grey,
    Purple,
    Ic,
    Ncf,
    Ice,
    Navy,
    Mint,
    Emerald,
    Orange,
    Rose,
    Dusk,
    Night,
    Wood,
    Wood2,
    Wood3,
    Wood4,
    Maple,
    Maple2,
    Leather,
    Marble,
    BlueMarble,
    Blue2,
    Blue3,
    Canvas,
    Metal,
    Olive,
    Stone,
    GreenPlastic,
    PurpleDiag,
    PinkPyramid,
    Horsey,
}

#[derive(Clone, Copy, Debug)]
pub struct BoardTheme {
    pub light: Color32,
    pub dark: Color32,
    /// Full-board image (8×8 squares, a1 dark) drawn instead of flat squares.
    pub texture: Option<&'static str>,
    pub last_move: Color32,
    pub selected: Color32,
    pub dot: Color32,
}

/// Board image textures from Lichess (see THIRD_PARTY_NOTICES.md).
const TEXTURES: &[(&str, &[u8])] = &[
    ("wood.jpg", include_bytes!("../assets/boards/wood.jpg")),
    ("wood2.jpg", include_bytes!("../assets/boards/wood2.jpg")),
    ("wood3.jpg", include_bytes!("../assets/boards/wood3.jpg")),
    ("wood4.jpg", include_bytes!("../assets/boards/wood4.jpg")),
    ("maple.jpg", include_bytes!("../assets/boards/maple.jpg")),
    ("maple2.jpg", include_bytes!("../assets/boards/maple2.jpg")),
    (
        "leather.jpg",
        include_bytes!("../assets/boards/leather.jpg"),
    ),
    ("marble.jpg", include_bytes!("../assets/boards/marble.jpg")),
    (
        "blue-marble.jpg",
        include_bytes!("../assets/boards/blue-marble.jpg"),
    ),
    ("blue2.jpg", include_bytes!("../assets/boards/blue2.jpg")),
    ("blue3.jpg", include_bytes!("../assets/boards/blue3.jpg")),
    (
        "canvas2.jpg",
        include_bytes!("../assets/boards/canvas2.jpg"),
    ),
    ("metal.jpg", include_bytes!("../assets/boards/metal.jpg")),
    ("olive.jpg", include_bytes!("../assets/boards/olive.jpg")),
    ("grey.jpg", include_bytes!("../assets/boards/grey.jpg")),
    (
        "green-plastic.png",
        include_bytes!("../assets/boards/green-plastic.png"),
    ),
    (
        "purple-diag.png",
        include_bytes!("../assets/boards/purple-diag.png"),
    ),
    (
        "pink-pyramid.png",
        include_bytes!("../assets/boards/pink-pyramid.png"),
    ),
    ("horsey.jpg", include_bytes!("../assets/boards/horsey.jpg")),
];

impl BoardThemeId {
    pub const ALL: [BoardThemeId; 37] = [
        BoardThemeId::Brown,
        BoardThemeId::Blue,
        BoardThemeId::Green,
        BoardThemeId::Tournament,
        BoardThemeId::Sand,
        BoardThemeId::Coral,
        BoardThemeId::Grey,
        BoardThemeId::Purple,
        BoardThemeId::Ic,
        BoardThemeId::Ncf,
        BoardThemeId::Ice,
        BoardThemeId::Navy,
        BoardThemeId::Mint,
        BoardThemeId::Emerald,
        BoardThemeId::Orange,
        BoardThemeId::Rose,
        BoardThemeId::Dusk,
        BoardThemeId::Night,
        BoardThemeId::Wood,
        BoardThemeId::Wood2,
        BoardThemeId::Wood3,
        BoardThemeId::Wood4,
        BoardThemeId::Maple,
        BoardThemeId::Maple2,
        BoardThemeId::Leather,
        BoardThemeId::Marble,
        BoardThemeId::BlueMarble,
        BoardThemeId::Blue2,
        BoardThemeId::Blue3,
        BoardThemeId::Canvas,
        BoardThemeId::Metal,
        BoardThemeId::Olive,
        BoardThemeId::Stone,
        BoardThemeId::GreenPlastic,
        BoardThemeId::PurpleDiag,
        BoardThemeId::PinkPyramid,
        BoardThemeId::Horsey,
    ];

    pub fn label(self) -> String {
        let (key, n): (&'static str, Option<&str>) = match self {
            BoardThemeId::Brown => ("Brown", None),
            BoardThemeId::Blue => ("Blue", None),
            BoardThemeId::Green => ("Green", None),
            BoardThemeId::Tournament => ("Tournament", None),
            BoardThemeId::Sand => ("Sand", None),
            BoardThemeId::Coral => ("Coral", None),
            BoardThemeId::Grey => ("Grey", None),
            BoardThemeId::Purple => ("Purple", None),
            BoardThemeId::Ic => ("IC", None),
            BoardThemeId::Ncf => ("NCF", None),
            BoardThemeId::Ice => ("Ice", None),
            BoardThemeId::Navy => ("Navy", None),
            BoardThemeId::Mint => ("Mint", None),
            BoardThemeId::Emerald => ("Emerald", None),
            BoardThemeId::Orange => ("Orange", None),
            BoardThemeId::Rose => ("Rose", None),
            BoardThemeId::Dusk => ("Dusk", None),
            BoardThemeId::Night => ("Night", None),
            BoardThemeId::Wood => ("Wood", None),
            BoardThemeId::Wood2 => ("Wood", Some("2")),
            BoardThemeId::Wood3 => ("Wood", Some("3")),
            BoardThemeId::Wood4 => ("Wood", Some("4")),
            BoardThemeId::Maple => ("Maple", None),
            BoardThemeId::Maple2 => ("Maple", Some("2")),
            BoardThemeId::Leather => ("Leather", None),
            BoardThemeId::Marble => ("Marble", None),
            BoardThemeId::BlueMarble => ("Blue marble", None),
            BoardThemeId::Blue2 => ("Blue", Some("2")),
            BoardThemeId::Blue3 => ("Blue", Some("3")),
            BoardThemeId::Canvas => ("Canvas", None),
            BoardThemeId::Metal => ("Metal", None),
            BoardThemeId::Olive => ("Olive", None),
            BoardThemeId::Stone => ("Stone", None),
            BoardThemeId::GreenPlastic => ("Green plastic", None),
            BoardThemeId::PurpleDiag => ("Purple diagonal", None),
            BoardThemeId::PinkPyramid => ("Pink pyramid", None),
            BoardThemeId::Horsey => ("Sketch", None),
        };
        match n {
            Some(n) => format!("{} {n}", tr(key)),
            None => tr(key).to_string(),
        }
    }

    pub fn theme(self) -> BoardTheme {
        let rgb = |(r, g, b): (u8, u8, u8)| Color32::from_rgb(r, g, b);
        let (light, dark, texture) = match self {
            BoardThemeId::Brown => (rgb((240, 217, 181)), rgb((181, 136, 99)), None),
            BoardThemeId::Blue => (rgb((222, 227, 230)), rgb((140, 162, 173)), None),
            BoardThemeId::Green => (rgb((255, 255, 221)), rgb((134, 166, 102)), None),
            BoardThemeId::Tournament => (rgb((238, 238, 210)), rgb((118, 150, 86)), None),
            BoardThemeId::Sand => (rgb((240, 228, 196)), rgb((200, 168, 110)), None),
            BoardThemeId::Coral => (rgb((177, 228, 185)), rgb((112, 162, 163)), None),
            BoardThemeId::Grey => (rgb((217, 217, 217)), rgb((154, 154, 154)), None),
            BoardThemeId::Purple => (rgb((232, 224, 240)), rgb((159, 144, 176)), None),
            // The colours of the Lichess "ic" and "ncf-board" boards.
            BoardThemeId::Ic => (rgb((236, 236, 236)), rgb((193, 193, 142)), None),
            BoardThemeId::Ncf => (rgb((187, 207, 255)), rgb((84, 119, 202)), None),
            BoardThemeId::Ice => (rgb((222, 234, 240)), rgb((122, 157, 178)), None),
            BoardThemeId::Navy => (rgb((220, 226, 236)), rgb((70, 96, 140)), None),
            BoardThemeId::Mint => (rgb((222, 241, 230)), rgb((118, 178, 148)), None),
            BoardThemeId::Emerald => (rgb((226, 236, 218)), rgb((74, 132, 100)), None),
            BoardThemeId::Orange => (rgb((250, 228, 196)), rgb((208, 139, 72)), None),
            BoardThemeId::Rose => (rgb((242, 220, 220)), rgb((190, 126, 138)), None),
            BoardThemeId::Dusk => (rgb((204, 186, 176)), rgb((112, 102, 122)), None),
            BoardThemeId::Night => (rgb((128, 136, 146)), rgb((68, 74, 84)), None),
            BoardThemeId::Wood => (
                rgb((215, 161, 91)),
                rgb((154, 88, 35)),
                Some("bytes://boards/wood.jpg"),
            ),
            BoardThemeId::Wood2 => (
                rgb((151, 124, 79)),
                rgb((121, 95, 48)),
                Some("bytes://boards/wood2.jpg"),
            ),
            BoardThemeId::Wood3 => (
                rgb((186, 173, 157)),
                rgb((129, 94, 56)),
                Some("bytes://boards/wood3.jpg"),
            ),
            BoardThemeId::Wood4 => (
                rgb((191, 158, 104)),
                rgb((134, 91, 54)),
                Some("bytes://boards/wood4.jpg"),
            ),
            BoardThemeId::Maple => (
                rgb((224, 190, 146)),
                rgb((180, 111, 60)),
                Some("bytes://boards/maple.jpg"),
            ),
            BoardThemeId::Maple2 => (
                rgb((222, 193, 151)),
                rgb((167, 114, 85)),
                Some("bytes://boards/maple2.jpg"),
            ),
            BoardThemeId::Leather => (
                rgb((206, 206, 198)),
                rgb((192, 140, 18)),
                Some("bytes://boards/leather.jpg"),
            ),
            BoardThemeId::Marble => (
                rgb((126, 150, 128)),
                rgb((97, 120, 97)),
                Some("bytes://boards/marble.jpg"),
            ),
            BoardThemeId::BlueMarble => (
                rgb((229, 224, 212)),
                rgb((148, 158, 174)),
                Some("bytes://boards/blue-marble.jpg"),
            ),
            BoardThemeId::Blue2 => (
                rgb((129, 151, 173)),
                rgb((96, 118, 138)),
                Some("bytes://boards/blue2.jpg"),
            ),
            BoardThemeId::Blue3 => (
                rgb((189, 200, 211)),
                rgb((68, 114, 163)),
                Some("bytes://boards/blue3.jpg"),
            ),
            BoardThemeId::Canvas => (
                rgb((203, 206, 223)),
                rgb((114, 130, 156)),
                Some("bytes://boards/canvas2.jpg"),
            ),
            BoardThemeId::Metal => (
                rgb((177, 177, 177)),
                rgb((119, 119, 119)),
                Some("bytes://boards/metal.jpg"),
            ),
            BoardThemeId::Olive => (
                rgb((173, 166, 148)),
                rgb((132, 124, 106)),
                Some("bytes://boards/olive.jpg"),
            ),
            BoardThemeId::Stone => (
                rgb((169, 169, 169)),
                rgb((136, 136, 136)),
                Some("bytes://boards/grey.jpg"),
            ),
            BoardThemeId::GreenPlastic => (
                rgb((241, 246, 178)),
                rgb((89, 147, 93)),
                Some("bytes://boards/green-plastic.png"),
            ),
            BoardThemeId::PurpleDiag => (
                rgb((227, 216, 238)),
                rgb((151, 123, 178)),
                Some("bytes://boards/purple-diag.png"),
            ),
            BoardThemeId::PinkPyramid => (
                rgb((237, 238, 191)),
                rgb((242, 117, 117)),
                Some("bytes://boards/pink-pyramid.png"),
            ),
            BoardThemeId::Horsey => (
                rgb((239, 222, 198)),
                rgb((151, 112, 81)),
                Some("bytes://boards/horsey.jpg"),
            ),
        };
        BoardTheme {
            light,
            dark,
            texture,
            last_move: Color32::from_rgba_unmultiplied(155, 199, 0, 105),
            selected: Color32::from_rgba_unmultiplied(20, 85, 30, 125),
            dot: Color32::from_rgba_unmultiplied(20, 85, 30, 110),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SoundSet {
    #[default]
    Sfx,
    Piano,
    Futuristic,
    Nes,
    Wood,
    Glass,
    Clicks,
}

impl SoundSet {
    pub const ALL: [SoundSet; 7] = [
        SoundSet::Sfx,
        SoundSet::Piano,
        SoundSet::Futuristic,
        SoundSet::Nes,
        SoundSet::Wood,
        SoundSet::Glass,
        SoundSet::Clicks,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SoundSet::Sfx => "SFX",
            SoundSet::Piano => tr("Piano"),
            SoundSet::Futuristic => tr("Futuristic"),
            SoundSet::Nes => "NES",
            SoundSet::Wood => tr("Wood"),
            SoundSet::Glass => tr("Glass"),
            SoundSet::Clicks => tr("Clicks"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    Move,
    Capture,
    Check,
    Checkmate,
}

pub fn sound_bytes(set: SoundSet, sound: Sound) -> &'static [u8] {
    macro_rules! s {
        ($set:literal, $f:literal) => {
            include_bytes!(concat!("../assets/sounds/", $set, "/", $f, ".ogg")).as_slice()
        };
    }
    // The Lichess sets have no checkmate sound of their own (it is the check sound there).
    match (set, sound) {
        (SoundSet::Sfx, Sound::Move) => s!("sfx", "Move"),
        (SoundSet::Sfx, Sound::Capture) => s!("sfx", "Capture"),
        (SoundSet::Sfx, Sound::Check | Sound::Checkmate) => s!("sfx", "Check"),
        (SoundSet::Piano, Sound::Move) => s!("piano", "Move"),
        (SoundSet::Piano, Sound::Capture) => s!("piano", "Capture"),
        (SoundSet::Piano, Sound::Check | Sound::Checkmate) => s!("piano", "Check"),
        (SoundSet::Futuristic, Sound::Move) => s!("futuristic", "Move"),
        (SoundSet::Futuristic, Sound::Capture) => s!("futuristic", "Capture"),
        (SoundSet::Futuristic, Sound::Check | Sound::Checkmate) => s!("futuristic", "Check"),
        (SoundSet::Nes, Sound::Move) => s!("nes", "Move"),
        (SoundSet::Nes, Sound::Capture) => s!("nes", "Capture"),
        (SoundSet::Nes, Sound::Check | Sound::Checkmate) => s!("nes", "Check"),
        (SoundSet::Wood, Sound::Move) => s!("wood", "Move"),
        (SoundSet::Wood, Sound::Capture) => s!("wood", "Capture"),
        (SoundSet::Wood, Sound::Check) => s!("wood", "Check"),
        (SoundSet::Wood, Sound::Checkmate) => s!("wood", "Checkmate"),
        (SoundSet::Glass, Sound::Move) => s!("glass", "Move"),
        (SoundSet::Glass, Sound::Capture) => s!("glass", "Capture"),
        (SoundSet::Glass, Sound::Check) => s!("glass", "Check"),
        (SoundSet::Glass, Sound::Checkmate) => s!("glass", "Checkmate"),
        (SoundSet::Clicks, Sound::Move) => s!("clicks", "Move"),
        (SoundSet::Clicks, Sound::Capture) => s!("clicks", "Capture"),
        (SoundSet::Clicks, Sound::Check) => s!("clicks", "Check"),
        (SoundSet::Clicks, Sound::Checkmate) => s!("clicks", "Checkmate"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_is_a_real_ogg_file() {
        // Lichess keeps some sounds as git symlinks; a copied link is a few bytes of text.
        for set in SoundSet::ALL {
            for s in [Sound::Move, Sound::Capture, Sound::Check, Sound::Checkmate] {
                let b = sound_bytes(set, s);
                assert!(b.starts_with(b"OggS") && b.len() > 1000, "{set:?} {s:?}");
            }
        }
    }

    #[test]
    fn every_piece_file_is_an_svg() {
        for set in PieceSet::ALL {
            for f in set.files() {
                let head = String::from_utf8_lossy(&f[..f.len().min(400)]);
                assert!(head.contains("<svg") || head.contains("<?xml"), "{set:?}");
            }
        }
    }
}
