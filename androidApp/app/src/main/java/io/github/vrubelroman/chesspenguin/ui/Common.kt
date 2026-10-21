package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.withStyle
import io.github.vrubelroman.chesspenguin.ChessPenguinApp
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.Notation
import io.github.vrubelroman.chesspenguin.data.DbEntry
import io.github.vrubelroman.chesspenguin.data.Source
import androidx.compose.ui.res.stringResource

@Composable
fun app(): ChessPenguinApp = LocalContext.current.applicationContext as ChessPenguinApp

/// 1248 → "1 248": digit groups apart, as on the desktop.
fun groupDigits(n: Int): String = "%,d".format(n).replace(',', ' ').replace(' ', ' ')

/// The name on the list: a sample keeps the name of the current language until renamed.
@Composable
fun displayName(e: DbEntry): String = when {
    e.source != Source.Sample || e.renamed -> e.name
    e.id == "sample-classics.pgn" -> stringResource(R.string.sample_classics)
    e.id == "sample-alekhine.pgn" -> stringResource(R.string.sample_alekhine)
    else -> e.name
}

/// The label of a database's format, for its badge (§10.6).
fun badge(e: DbEntry): String {
    val ext = e.fileName.substringAfterLast('.', "").lowercase()
    return when (ext) {
        "zip" -> "ZIP"
        "7z" -> "7Z"
        "cbv", "cbh", "cbz" -> "CB"
        else -> "PGN"
    }
}

@Composable
fun FormatBadge(text: String, error: Boolean = false) {
    Box(
        Modifier
            .size(40.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(if (error) MaterialTheme.colorScheme.errorContainer else MaterialTheme.colorScheme.primaryContainer),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text,
            style = MaterialTheme.typography.labelSmall,
            color = if (error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onPrimaryContainer,
        )
    }
}

/// "Carlsen, Magnus" → "Carlsen M."; other names stay as in the file (§6.2).
fun shortName(name: String): String {
    val n = name.trim()
    if (n.isEmpty() || n == "?") return "?"
    val comma = n.indexOf(',')
    if (comma <= 0) return n
    val surname = n.substring(0, comma).trim()
    val initials = n.substring(comma + 1).trim().split(' ', '-').filter { it.isNotEmpty() }.joinToString(" ") { "${it.first()}." }
    return if (initials.isEmpty()) surname else "$surname $initials"
}

/// "1/2-1/2" → "½-½".
fun shortResult(result: String): String = when (result) {
    "1/2-1/2" -> "½-½"
    "" -> "*"
    else -> result
}

/// The year of a PGN date ("1927.??.??" → "1927"), or nothing.
fun year(date: String): String = date.take(4).takeIf { y -> y.length == 4 && y.all { it.isDigit() } } ?: ""

/// The parts that are known, joined with " · " (empty values and "?" left out).
fun joinKnown(vararg parts: String): String = parts.map { it.trim() }.filter { it.isNotEmpty() && it != "?" }.joinToString(" · ")

/// The figurines of the desktop app (OFL, apps/desktop/assets/fonts/OFL.txt).
val Figurines = FontFamily(Font(R.font.figurines))

/// King, queen, rook, bishop, knight in a language, as on the desktop (san.rs).
private fun localLetters(lang: String): List<String>? = when (lang) {
    "ru" -> listOf("Кр", "Ф", "Л", "С", "К")
    "de" -> listOf("K", "D", "T", "L", "S")
    "fr" -> listOf("R", "D", "T", "F", "C")
    "es", "it" -> listOf("R", "D", "T", "A", "C")
    else -> null
}

/// A SAN move as shown: the piece letter at the start and after "=" replaced; the
/// game itself stays in Latin SAN.
fun displaySan(san: String, mode: Notation, lang: String): AnnotatedString {
    val local = localLetters(lang)
    if (mode == Notation.Latin || (mode == Notation.Local && local == null)) return AnnotatedString(san)
    return buildAnnotatedString {
        san.forEachIndexed { i, c ->
            val k = "KQRBN".indexOf(c)
            val piece = k >= 0 && (i == 0 || san[i - 1] == '=')
            when {
                !piece -> append(c)
                mode == Notation.Figurines -> withStyle(SpanStyle(fontFamily = Figurines)) { append("♔♕♖♗♘"[k]) }
                else -> append(local!![k])
            }
        }
    }
}

/// A line like "14.Nd5 Bxd5 15.exd5": each move shown with [displaySan].
fun displayLine(text: String, mode: Notation, lang: String): AnnotatedString {
    if (mode == Notation.Latin) return AnnotatedString(text)
    return buildAnnotatedString {
        text.split(' ').forEachIndexed { i, word ->
            if (i > 0) append(' ')
            val cut = word.indexOfLast { it == '.' || it == '…' } + 1
            append(word.substring(0, cut))
            append(displaySan(word.substring(cut), mode, lang))
        }
    }
}

/// The language of the interface ("ru", "en", …).
@Composable
fun uiLanguage(): String = LocalConfiguration.current.locales[0].language
