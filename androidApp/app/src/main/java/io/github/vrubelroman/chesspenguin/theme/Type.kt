package io.github.vrubelroman.chesspenguin.theme

import androidx.compose.material3.Typography
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

// The system font (Roboto on most phones), the sizes of §10.2.
val Typography = Typography().run {
    copy(
        titleLarge = titleLarge.copy(fontSize = 22.sp, lineHeight = 28.sp, fontWeight = FontWeight.Normal),
        bodyLarge = bodyLarge.copy(fontSize = 16.sp, lineHeight = 24.sp),
        bodyMedium = bodyMedium.copy(fontSize = 14.sp, lineHeight = 20.sp),
        labelLarge = labelLarge.copy(fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium),
        labelSmall = labelSmall.copy(fontSize = 12.sp, lineHeight = 16.sp, fontWeight = FontWeight.Medium),
    )
}

/// Digits that keep their width (results, counters).
val Tabular = TextStyle(fontFeatureSettings = "tnum")
