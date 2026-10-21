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

/// Colors of the notation and the board that stay ours whatever the theme.
data class ChessColors(
    val dark: Boolean,
    val currentMove: Color,
    val comment: Color,
    val lightSquare: Color,
    val darkSquare: Color,
    val lastMove: Color,
)

val LightChess = ChessColors(
    dark = false,
    currentMove = Color(0xFF90D1FF),
    comment = Color(0xFF2A5B8A),
    lightSquare = Color(0xFFFFFFDD),
    darkSquare = Color(0xFF86A666),
    lastMove = Color(0x699BC700),
)

val DarkChess = LightChess.copy(
    dark = true,
    currentMove = Color(0xFF005C80),
    comment = Color(0xFF9EC4E0),
)

/// Move assessments in the colors of Lichess (as on the desktop). On a light background
/// they are darkened to stay readable (contrast at least 4.5:1).
fun nagColor(nag: Int, dark: Boolean): Color? = if (dark) {
    when (nag) {
        1 -> Color(0xFF629C2E) // !
        2 -> Color(0xFFE68F00) // ?
        3 -> Color(0xFF22AC9A) // !!
        4 -> Color(0xFFDF5353) // ??
        5 -> Color(0xFFEA45D8) // !?
        6 -> Color(0xFF56B4E9) // ?!
        else -> null
    }
} else {
    when (nag) {
        1 -> Color(0xFF4E7F22)
        2 -> Color(0xFFA35F00)
        3 -> Color(0xFF12806F)
        4 -> Color(0xFFC23B3B)
        5 -> Color(0xFFB0259F)
        6 -> Color(0xFF1F72A8)
        else -> null
    }
}
