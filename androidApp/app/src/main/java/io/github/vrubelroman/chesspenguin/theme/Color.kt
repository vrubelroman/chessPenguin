package io.github.vrubelroman.chesspenguin.theme

import androidx.compose.ui.graphics.Color

// The colors of the desktop app and the website (docs/design/android-ui.md §10.3).

val LightBackground = Color(0xFFFFFFFF)
val LightContainerLow = Color(0xFFF5F6F7)
val LightContainer = Color(0xFFEEF0F2)
val LightContainerHigh = Color(0xFFE8EAED)
val LightOnSurface = Color(0xFF1C1D1F)
val LightOnSurfaceVariant = Color(0xFF5D636B)
val LightOutline = Color(0xFFC4C8CD)
val LightOutlineVariant = Color(0xFFE1E3E6)
val LightPrimary = Color(0xFF1F6FA8)
val LightPrimaryContainer = Color(0xFFD6E8F6)
val LightOnPrimaryContainer = Color(0xFF1A5F91)
val LightError = Color(0xFFC42B1C)
val LightErrorContainer = Color(0xFFFCE4E1)

val DarkBackground = Color(0xFF161718)
val DarkContainerLow = Color(0xFF1F2022)
val DarkContainer = Color(0xFF232427)
val DarkContainerHigh = Color(0xFF2A2C2F)
val DarkOnSurface = Color(0xFFE8E8E8)
val DarkOnSurfaceVariant = Color(0xFFA3A7AD)
val DarkOutline = Color(0xFF4A4D52)
val DarkOutlineVariant = Color(0xFF2E3033)
val DarkPrimary = Color(0xFF62AEE6)
val DarkOnPrimary = Color(0xFF0B2A40)
val DarkPrimaryContainer = Color(0xFF1A4566)
val DarkError = Color(0xFFF28B82)
val DarkErrorContainer = Color(0xFF5C1A14)

/// Colors of the notation and the board that stay ours whatever the theme (the
/// squares are the chosen board's, data/BoardStyle.kt).
data class ChessColors(
    val dark: Boolean,
    val currentMove: Color,
    val comment: Color,
    val lastMove: Color,
)

val LightChess = ChessColors(
    dark = false,
    currentMove = Color(0xFF90D1FF),
    comment = Color(0xFF2A5B8A),
    lastMove = Color(0x699BC700),
)

val DarkChess = LightChess.copy(
    dark = true,
    currentMove = Color(0xFF005C80),
    comment = Color(0xFF9EC4E0),
)

/// Move assessments in the owner's palette (chess.com's), as on the desktop. On a light
/// background they are darkened to stay readable (contrast at least 4.5:1).
fun nagColor(nag: Int, dark: Boolean): Color? = if (dark) {
    when (nag) {
        1 -> Color(0xFF81B64C) // !
        2 -> Color(0xFFFFA459) // ?
        3 -> Color(0xFF26C2A3) // !!
        4 -> Color(0xFFFA412D) // ??
        5 -> Color(0xFFEA45D8) // !?
        6 -> Color(0xFFF7C631) // ?!
        else -> null
    }
} else {
    when (nag) {
        1 -> Color(0xFF5B8135)
        2 -> Color(0xFFC25700)
        3 -> Color(0xFF1A846F)
        4 -> Color(0xFFD2301C)
        5 -> Color(0xFFB0259F)
        6 -> Color(0xFF937006)
        else -> null
    }
}

/// The kinds of databases, each in its own colour on its badge (android-lists.md §5.1).
enum class DbKind { Pgn, ChessBase, Archive, Online, Own }

/// A badge's background and what is written on it.
data class KindColor(val container: Color, val content: Color)

/// The colour of a kind of database, from Catppuccin (the owner's choice): Mocha in the
/// dark theme, Latte in the light one, on a tint of the same hue (contrast at least 5:1).
fun kindColor(kind: DbKind, dark: Boolean): KindColor = if (dark) {
    when (kind) {
        DbKind.Pgn -> KindColor(Color(0xFF384660), Color(0xFF89B4FA)) // blue
        DbKind.ChessBase -> KindColor(Color(0xFF55443B), Color(0xFFFAB387)) // peach
        DbKind.Archive -> KindColor(Color(0xFF4A3F5E), Color(0xFFCBA6F7)) // mauve
        DbKind.Online -> KindColor(Color(0xFF3A5250), Color(0xFF94E2D5)) // teal
        DbKind.Own -> KindColor(Color(0xFF3F523F), Color(0xFFA6E3A1)) // green
    }
} else {
    when (kind) {
        DbKind.Pgn -> KindColor(Color(0xFFDFEAFE), Color(0xFF1957D0))
        DbKind.ChessBase -> KindColor(Color(0xFFFFE9DD), Color(0xFFA54107))
        DbKind.Archive -> KindColor(Color(0xFFEEE3FD), Color(0xFF7A33D7))
        DbKind.Online -> KindColor(Color(0xFFDFF0F1), Color(0xFF116D73))
        DbKind.Own -> KindColor(Color(0xFFE4F2E1), Color(0xFF2D701E))
    }
}
