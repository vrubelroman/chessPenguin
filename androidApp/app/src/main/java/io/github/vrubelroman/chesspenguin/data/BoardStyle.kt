package io.github.vrubelroman.chesspenguin.data

import androidx.annotation.StringRes
import androidx.compose.ui.graphics.Color
import io.github.vrubelroman.chesspenguin.R

/// The piece sets of the desktop app (apps/desktop/src/assets.rs), drawn from
/// `assets/pieces/<dir>`; the name is the set's own, as on Lichess.
enum class PieceSet(val dir: String) {
    Cburnett("cburnett"),
    Merida("merida"),
    Mpchess("mpchess"),
    Chessnut("chessnut"),
    Fantasy("fantasy"),
    Celtic("celtic"),
    Spatial("spatial"),
    Rhosgfx("rhosgfx"),
    Shapes("shapes"),
    KiwenSuwi("kiwen-suwi"),
    Firi("firi"),
    Pixel("pixel"),
    Letter("letter"),
    Pirouetti("pirouetti"),
    Governor("governor"),
    Kosal("kosal"),
    Metaltops("metaltops"),
}

/// The boards of the desktop app (assets.rs `BoardThemeId`): two colors, and for some a
/// full-board image (`assets/boards/<file>`, 8×8 squares, a1 dark) drawn instead of them.
/// The colors stay for the coordinates and the previews.
enum class BoardTheme(
    @StringRes val label: Int,
    val number: Int?,
    val light: Color,
    val dark: Color,
    val texture: String? = null,
) {
    Brown(R.string.board_brown, null, rgb(240, 217, 181), rgb(181, 136, 99)),
    Blue(R.string.board_blue, null, rgb(222, 227, 230), rgb(140, 162, 173)),
    Green(R.string.board_green, null, rgb(255, 255, 221), rgb(134, 166, 102)),
    Tournament(R.string.board_tournament, null, rgb(238, 238, 210), rgb(118, 150, 86)),
    Sand(R.string.board_sand, null, rgb(240, 228, 196), rgb(200, 168, 110)),
    Coral(R.string.board_coral, null, rgb(177, 228, 185), rgb(112, 162, 163)),
    Grey(R.string.board_grey, null, rgb(217, 217, 217), rgb(154, 154, 154)),
    Purple(R.string.board_purple, null, rgb(232, 224, 240), rgb(159, 144, 176)),
    Ic(R.string.board_ic, null, rgb(236, 236, 236), rgb(193, 193, 142)),
    Ncf(R.string.board_ncf, null, rgb(187, 207, 255), rgb(84, 119, 202)),
    Ice(R.string.board_ice, null, rgb(222, 234, 240), rgb(122, 157, 178)),
    Navy(R.string.board_navy, null, rgb(220, 226, 236), rgb(70, 96, 140)),
    Mint(R.string.board_mint, null, rgb(222, 241, 230), rgb(118, 178, 148)),
    Emerald(R.string.board_emerald, null, rgb(226, 236, 218), rgb(74, 132, 100)),
    Orange(R.string.board_orange, null, rgb(250, 228, 196), rgb(208, 139, 72)),
    Rose(R.string.board_rose, null, rgb(242, 220, 220), rgb(190, 126, 138)),
    Dusk(R.string.board_dusk, null, rgb(204, 186, 176), rgb(112, 102, 122)),
    Night(R.string.board_night, null, rgb(128, 136, 146), rgb(68, 74, 84)),
    Wood(R.string.board_wood, null, rgb(215, 161, 91), rgb(154, 88, 35), "wood.jpg"),
    Wood2(R.string.board_wood, 2, rgb(151, 124, 79), rgb(121, 95, 48), "wood2.jpg"),
    Wood3(R.string.board_wood, 3, rgb(186, 173, 157), rgb(129, 94, 56), "wood3.jpg"),
    Wood4(R.string.board_wood, 4, rgb(191, 158, 104), rgb(134, 91, 54), "wood4.jpg"),
    Maple(R.string.board_maple, null, rgb(224, 190, 146), rgb(180, 111, 60), "maple.jpg"),
    Maple2(R.string.board_maple, 2, rgb(222, 193, 151), rgb(167, 114, 85), "maple2.jpg"),
    Leather(R.string.board_leather, null, rgb(206, 206, 198), rgb(192, 140, 18), "leather.jpg"),
    Marble(R.string.board_marble, null, rgb(126, 150, 128), rgb(97, 120, 97), "marble.jpg"),
    BlueMarble(R.string.board_blue_marble, null, rgb(229, 224, 212), rgb(148, 158, 174), "blue-marble.jpg"),
    Blue2(R.string.board_blue, 2, rgb(129, 151, 173), rgb(96, 118, 138), "blue2.jpg"),
    Blue3(R.string.board_blue, 3, rgb(189, 200, 211), rgb(68, 114, 163), "blue3.jpg"),
    Canvas(R.string.board_canvas, null, rgb(203, 206, 223), rgb(114, 130, 156), "canvas2.jpg"),
    Metal(R.string.board_metal, null, rgb(177, 177, 177), rgb(119, 119, 119), "metal.jpg"),
    Olive(R.string.board_olive, null, rgb(173, 166, 148), rgb(132, 124, 106), "olive.jpg"),
    Stone(R.string.board_stone, null, rgb(169, 169, 169), rgb(136, 136, 136), "grey.jpg"),
    GreenPlastic(R.string.board_green_plastic, null, rgb(241, 246, 178), rgb(89, 147, 93), "green-plastic.png"),
    PurpleDiag(R.string.board_purple_diag, null, rgb(227, 216, 238), rgb(151, 123, 178), "purple-diag.png"),
    PinkPyramid(R.string.board_pink_pyramid, null, rgb(237, 238, 191), rgb(242, 117, 117), "pink-pyramid.png"),
    Horsey(R.string.board_sketch, null, rgb(239, 222, 198), rgb(151, 112, 81), "horsey.jpg"),
}

private fun rgb(r: Int, g: Int, b: Int) = Color(r, g, b)
