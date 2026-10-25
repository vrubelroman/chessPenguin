package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.border
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextOverflow
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
import io.github.vrubelroman.chesspenguin.theme.Tabular
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

/// The format of a database in words, as the core names it once it is opened.
fun formatName(e: DbEntry): String = when (e.fileName.substringAfterLast('.', "").lowercase()) {
    "cbh" -> "ChessBase"
    "cbv" -> "ChessBase CBV"
    "zip" -> "ZIP"
    "7z" -> "7z"
    else -> "PGN"
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

/// A player's name (android-ui.md §7.11): in full while it fits, else "Surname F.", else
/// cut with "…". `abbreviate = false` keeps the text as it is (a study's title for White).
@Composable
fun PlayerName(
    name: String,
    style: TextStyle,
    modifier: Modifier = Modifier,
    color: Color = Color.Unspecified,
    maxLines: Int = 1,
    abbreviate: Boolean = true,
) {
    val measurer = rememberTextMeasurer()
    BoxWithConstraints(modifier) {
        val text = if (abbreviate && measurer.measure(name, style, softWrap = false).size.width > constraints.maxWidth) shortName(name) else name
        Text(text, style = style, color = color, maxLines = maxLines, overflow = TextOverflow.Ellipsis)
    }
}

/// A player's share of the result: "1", "0", "½", or null (unknown, unfinished).
fun sideResult(result: String, white: Boolean): String? = when (result) {
    "1-0" -> if (white) "1" else "0"
    "0-1" -> if (white) "0" else "1"
    "1/2-1/2" -> "½"
    else -> null
}

/// The rating of a player, or nothing ("0", "?", "").
fun elo(value: String): String = value.trim().takeIf { v -> v.toIntOrNull()?.let { it > 0 } == true } ?: ""

/// The colour of a side: a small disc (white with a rim, or black).
@Composable
fun SideDisc(white: Boolean) {
    Box(
        Modifier.size(12.dp).clip(CircleShape)
            .background(if (white) Color(0xFFF5F5F0) else Color(0xFF26282B))
            .border(1.dp, MaterialTheme.colorScheme.outline, CircleShape),
    )
}

/// One player's line: name (giving way), Elo, the share of the result on the right.
@Composable
fun PlayerLine(
    name: String,
    elo: String,
    result: String?,
    style: TextStyle,
    modifier: Modifier = Modifier,
    disc: Boolean? = null,
    nameColor: Color = Color.Unspecified,
    maxLines: Int = 1,
    abbreviate: Boolean = true,
    /// What the player is doing now («thinking…» of Maia, android-play.md §31.3).
    status: String? = null,
) {
    Row(modifier, verticalAlignment = Alignment.CenterVertically) {
        if (disc != null) {
            SideDisc(disc)
            Spacer(Modifier.width(8.dp))
        }
        Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
            PlayerName(name, style, Modifier.weight(1f, fill = false), nameColor, maxLines, abbreviate)
            if (elo.isNotEmpty()) {
                Spacer(Modifier.width(8.dp))
                Text(elo, style = MaterialTheme.typography.bodyMedium.merge(Tabular), color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
            }
        }
        if (status != null) {
            androidx.compose.material3.CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
            Text(status, Modifier.padding(start = 6.dp), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
        }
        Text(
            result ?: "",
            style = style.merge(Tabular),
            color = if (result == "1") MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant,
            fontWeight = if (result == "1") androidx.compose.ui.text.font.FontWeight.Medium else null,
            textAlign = androidx.compose.ui.text.style.TextAlign.End,
            maxLines = 1,
            modifier = Modifier.width(24.dp),
        )
    }
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
fun localLetters(lang: String): List<String>? = when (lang) {
    "ru" -> listOf("Кр", "Ф", "Л", "С", "К")
    "de" -> listOf("K", "D", "T", "L", "S")
    "fr" -> listOf("R", "D", "T", "F", "C")
    "es", "it" -> listOf("R", "D", "T", "A", "C")
    "pt" -> listOf("R", "D", "T", "B", "C")
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
