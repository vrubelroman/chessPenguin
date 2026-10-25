package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import io.github.vrubelroman.chesspenguin.R
import uniffi.mobile_api.GameData
import uniffi.mobile_api.Tag

/// "2026.10.26", "2026.10", "2026", "26.10.2026" (as people write it) → the PGN date
/// ("2026.10.26", "2026.10.??", "2026.??.??"); "" stays ""; null when it is not a date.
fun pgnDateOf(text: String): String? {
    val t = text.trim()
    if (t.isEmpty()) return ""
    Regex("""(\d{4})(?:[.\-/](\d{1,2}|\?\?))?(?:[.\-/](\d{1,2}|\?\?))?""").matchEntire(t)?.let { m ->
        val (y, mo, d) = m.destructured
        return listOf(y, part(mo, 12) ?: return null, part(d, 31) ?: return null).joinToString(".").takeIf(::realDay)
    }
    Regex("""(\d{1,2})[.\-/](\d{1,2})[.\-/](\d{4})""").matchEntire(t)?.let { m ->
        val (d, mo, y) = m.destructured
        return listOf(y, part(mo, 12) ?: return null, part(d, 31) ?: return null).joinToString(".").takeIf(::realDay)
    }
    return null
}

private fun part(p: String, max: Int): String? = when {
    p.isEmpty() || p == "??" -> "??"
    p.toInt() in 1..max -> p.padStart(2, '0')
    else -> null
}

/// A day that the month has (31.02 is not one).
private fun realDay(date: String): Boolean {
    val (y, m, d) = date.split('.').map { it.toIntOrNull() } + listOf(null, null, null)
    if (y == null || m == null || d == null) return true
    return runCatching { java.time.LocalDate.of(y, m, d) }.isSuccess
}

private val RESULTS = listOf("1-0", "1/2-1/2", "0-1", "*")

/// The game's data (android-play.md §30.5): players and ratings, event, place, date,
/// round, result. Nothing is required; `done` gets the tags (empty: removed) and result.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun GameDataForm(game: GameData, doneLabel: String, onDone: (List<Tag>, String) -> Unit, onDismiss: () -> Unit) {
    fun tag(name: String) = game.tags.firstOrNull { it.name == name }?.value?.takeIf { it != "?" && !it.startsWith("????") }.orEmpty()
    var white by rememberSaveable { mutableStateOf(tag("White")) }
    var whiteElo by rememberSaveable { mutableStateOf(tag("WhiteElo")) }
    var black by rememberSaveable { mutableStateOf(tag("Black")) }
    var blackElo by rememberSaveable { mutableStateOf(tag("BlackElo")) }
    var event by rememberSaveable { mutableStateOf(tag("Event")) }
    var site by rememberSaveable { mutableStateOf(tag("Site")) }
    var date by rememberSaveable { mutableStateOf(tag("Date")) }
    var round by rememberSaveable { mutableStateOf(tag("Round")) }
    var result by rememberSaveable { mutableStateOf(game.result.takeIf { it in RESULTS } ?: "*") }
    val start = listOf(white, whiteElo, black, blackElo, event, site, date, round, result)
    val initial = rememberSaveable { start.joinToString("\u0000") }
    val changed = start.joinToString("\u0000") != initial
    var asking by rememberSaveable { mutableStateOf(false) }
    val pgnDate = pgnDateOf(date)
    fun close() = if (changed) asking = true else onDismiss()
    Dialog(onDismissRequest = ::close, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        Scaffold(
            topBar = {
                TopAppBar(
                    navigationIcon = { IconButton(onClick = ::close) { Icon(Icons.Rounded.Close, stringResource(R.string.close)) } },
                    title = { Text(stringResource(R.string.game_data)) },
                    actions = {
                        TextButton(enabled = pgnDate != null, onClick = {
                            onDone(
                                listOf(
                                    Tag("White", white.trim()), Tag("WhiteElo", whiteElo.trim()),
                                    Tag("Black", black.trim()), Tag("BlackElo", blackElo.trim()),
                                    Tag("Event", event.trim()), Tag("Site", site.trim()),
                                    Tag("Date", pgnDate ?: ""), Tag("Round", round.trim()),
                                ),
                                result,
                            )
                        }) { Text(doneLabel) }
                    },
                )
            },
        ) { padding ->
            Column(
                Modifier.fillMaxSize().padding(padding).imePadding().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                val digits = KeyboardOptions(keyboardType = KeyboardType.Number)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(white, { white = it }, Modifier.weight(1f), label = { Text(stringResource(R.string.tag_white)) }, singleLine = true)
                    OutlinedTextField(whiteElo, { v -> whiteElo = v.filter(Char::isDigit).take(4) }, Modifier.width(88.dp), label = { Text(stringResource(R.string.elo)) }, singleLine = true, keyboardOptions = digits)
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(black, { black = it }, Modifier.weight(1f), label = { Text(stringResource(R.string.tag_black)) }, singleLine = true)
                    OutlinedTextField(blackElo, { v -> blackElo = v.filter(Char::isDigit).take(4) }, Modifier.width(88.dp), label = { Text(stringResource(R.string.elo)) }, singleLine = true, keyboardOptions = digits)
                }
                OutlinedTextField(event, { event = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(R.string.tag_event)) }, singleLine = true)
                OutlinedTextField(site, { site = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(R.string.tag_site)) }, singleLine = true)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(
                        date, { date = it }, Modifier.weight(1f),
                        label = { Text(stringResource(R.string.tag_date)) },
                        placeholder = { Text(stringResource(R.string.date_hint)) },
                        singleLine = true,
                        isError = pgnDate == null,
                        supportingText = if (pgnDate == null) { { Text(stringResource(R.string.date_wrong)) } } else null,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Phone),
                    )
                    OutlinedTextField(round, { round = it }, Modifier.width(88.dp), label = { Text(stringResource(R.string.tag_round)) }, singleLine = true)
                }
                Text(stringResource(R.string.tag_result), style = MaterialTheme.typography.labelLarge, modifier = Modifier.padding(top = 8.dp))
                SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth().padding(bottom = 24.dp)) {
                    RESULTS.forEachIndexed { i, r ->
                        SegmentedButton(
                            selected = result == r,
                            onClick = { result = r },
                            shape = SegmentedButtonDefaults.itemShape(i, RESULTS.size),
                        ) { Text(if (r == "1/2-1/2") "½-½" else r) }
                    }
                }
            }
        }
        if (asking) {
            AlertDialog(
                onDismissRequest = { asking = false },
                title = { Text(stringResource(R.string.discard_changes)) },
                confirmButton = {
                    TextButton(onClick = {
                        asking = false
                        onDismiss()
                    }) { Text(stringResource(R.string.discard)) }
                },
                dismissButton = { TextButton(onClick = { asking = false }) { Text(stringResource(R.string.keep_editing)) } },
            )
        }
    }
}
