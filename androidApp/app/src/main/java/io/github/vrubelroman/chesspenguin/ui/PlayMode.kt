package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.Undo
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Flag
import androidx.compose.material.icons.rounded.Handshake
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.SwapVert
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.scale
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.PlayMeta
import uniffi.mobile_api.GameData
import uniffi.mobile_api.GameEnding
import uniffi.mobile_api.MaiaPlayer
import uniffi.mobile_api.ResignJudge

/// The key of the game against Maia in hand (a game screen without a database).
const val PLAY_GAME = "play"

/// How a game against Maia ended: the result and what the strip says.
data class PlayOver(val result: String, val heading: Int, val detail: Int?)

/// A game against Maia being played (android-play.md §31), kept while the phone turns.
class PlayRun(val meta: PlayMeta) {
    var player: MaiaPlayer? = null
    var judge: ResignJudge? = null
    val thinking = mutableStateOf(false)
    /// Maia offers to resign (her position hopeless two turns in a row).
    val offer = mutableStateOf(false)
    val over = mutableStateOf<PlayOver?>(null)
    /// Maia failed to start: why.
    val failure = mutableStateOf<String?>(null)
    /// «Play to the end»: no more offers in this game.
    var playOn = false
    var hopeless = 0u
    /// Bumped by a take-back: Maia's answer for the old position is dropped.
    var generation = 0

    fun close() {
        player?.close()
        judge?.close()
        player = null
        judge = null
    }
}

/// The last node of the main line.
fun lineEnd(g: GameData): Int {
    var n = 0
    while (true) n = g.nodes[n].children.firstOrNull()?.toInt() ?: return n
}

/// Half-moves from the start to node `id`.
fun depth(g: GameData, id: Int): Int {
    var d = 0
    var n = g.nodes.getOrNull(id) ?: return 0
    while (true) {
        n = g.nodes.getOrNull(n.parent?.toInt() ?: return d) ?: return d
        d++
    }
}

fun whiteToMove(fen: String) = fen.substringAfter(' ').startsWith("w")

/// The end of a game by the rules, from the user's side.
fun overBy(ending: GameEnding, userWhite: Boolean): PlayOver = when (ending) {
    is GameEnding.Checkmate -> PlayOver(
        if (ending.whiteWon) "1-0" else "0-1",
        if (ending.whiteWon == userWhite) R.string.play_won else R.string.play_lost,
        R.string.end_mate,
    )
    GameEnding.Stalemate -> PlayOver("1/2-1/2", R.string.play_draw, R.string.end_stalemate)
    GameEnding.InsufficientMaterial -> PlayOver("1/2-1/2", R.string.play_draw, R.string.end_material)
    GameEnding.Threefold -> PlayOver("1/2-1/2", R.string.play_draw, R.string.end_threefold)
    GameEnding.FiftyMoves -> PlayOver("1/2-1/2", R.string.play_draw, R.string.end_fifty)
}

/// The bottom bar during a game (§31.3): take back, resign (abort before the first
/// moves), flip, a move back and forward.
@Composable
fun PlayControls(
    canTakeBack: Boolean,
    started: Boolean,
    canBack: Boolean,
    canForward: Boolean,
    onTakeBack: () -> Unit,
    onResign: () -> Unit,
    onDraw: () -> Unit,
    onAbort: () -> Unit,
    onFlip: () -> Unit,
    onBack: () -> Unit,
    onForward: () -> Unit,
) {
    Column {
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
            Row(Modifier.fillMaxWidth().navigationBarsPadding().height(72.dp), verticalAlignment = Alignment.CenterVertically) {
                LabeledButton(Icons.AutoMirrored.Rounded.Undo, stringResource(R.string.play_take_back).removePrefix("↶ "), canTakeBack, Modifier.weight(1f), onTakeBack)
                if (started) {
                    LabeledButton(Icons.Rounded.Flag, stringResource(R.string.play_resign).removePrefix("⚑ "), true, Modifier.weight(1f), onResign)
                    LabeledButton(Icons.Rounded.Handshake, stringResource(R.string.play_offer_draw), true, Modifier.weight(1f), onDraw)
                } else {
                    LabeledButton(Icons.Rounded.Close, stringResource(R.string.play_abort), true, Modifier.weight(1f), onAbort)
                }
                IconButton(onClick = onFlip, modifier = Modifier.weight(0.8f)) { Icon(Icons.Rounded.SwapVert, stringResource(R.string.flip)) }
                IconButton(onClick = onBack, enabled = canBack, modifier = Modifier.weight(1f)) {
                    // The forward arrow turned round, as the game screen's.
                    Icon(Icons.Rounded.PlayArrow, stringResource(R.string.move_back), Modifier.scale(-1f, 1f))
                }
                IconButton(onClick = onForward, enabled = canForward, modifier = Modifier.weight(1f)) { Icon(Icons.Rounded.PlayArrow, stringResource(R.string.move_forward)) }
            }
        }
    }
}

@Composable
private fun LabeledButton(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, enabled: Boolean, modifier: Modifier, onClick: () -> Unit) {
    androidx.compose.material3.TextButton(onClick = onClick, enabled = enabled, modifier = modifier.height(64.dp), contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 2.dp)) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Icon(icon, null)
            // Six buttons in a row: a long word goes to a second line, not cut.
            Text(label, style = MaterialTheme.typography.labelSmall, maxLines = 2, overflow = TextOverflow.Ellipsis, textAlign = androidx.compose.ui.text.style.TextAlign.Center)
        }
    }
}

/// Maia offers to resign (§31.3): accept the win, or play on.
@Composable
fun OfferCard(onAccept: () -> Unit, onPlayOn: () -> Unit) {
    Card {
        Text(stringResource(R.string.play_maia_resigns), style = MaterialTheme.typography.titleMedium)
        Text(stringResource(R.string.play_her_position_lost), style = MaterialTheme.typography.bodyMedium)
        Row(Modifier.fillMaxWidth().padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(onClick = onAccept, modifier = Modifier.weight(1f)) { Text(stringResource(R.string.play_accept_win)) }
            OutlinedButton(onClick = onPlayOn, modifier = Modifier.weight(1f)) { Text(stringResource(R.string.play_on_short)) }
        }
    }
}

/// The end of the game (§31.4): the result, then analyse, rematch, save.
@Composable
fun ResultCard(over: PlayOver, saved: Boolean, onAnalyse: () -> Unit, onRematch: () -> Unit, onSave: () -> Unit, onClose: () -> Unit) {
    Card {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                (if (over.result == "1/2-1/2") "½-½" else over.result) + " · " + stringResource(over.heading) + (over.detail?.let { " · " + stringResource(it) } ?: ""),
                Modifier.weight(1f),
                style = MaterialTheme.typography.titleMedium,
            )
            IconButton(onClick = onClose) { Icon(Icons.Rounded.Close, stringResource(R.string.close)) }
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val pad = androidx.compose.foundation.layout.PaddingValues(horizontal = 8.dp)
            Button(onClick = onAnalyse, modifier = Modifier.weight(1f), contentPadding = pad) { Text(stringResource(R.string.play_analyse_short), maxLines = 1, overflow = TextOverflow.Ellipsis) }
            OutlinedButton(onClick = onRematch, modifier = Modifier.weight(1f), contentPadding = pad) {
                Text(stringResource(R.string.play_rematch).removePrefix("↻ "), maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            OutlinedButton(onClick = onSave, enabled = !saved, modifier = Modifier.weight(1f), contentPadding = pad) {
                Text(stringResource(if (saved) R.string.play_saved else R.string.play_save), maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

@Composable
private fun Card(content: @Composable () -> Unit) {
    Column(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 8.dp)
            .background(MaterialTheme.colorScheme.surfaceContainerHigh, RoundedCornerShape(12.dp))
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) { content() }
}

/// Puts a new game against Maia in hand (android-play.md §31): `session` holds its moves
/// so far (none, or those of the game it goes on from: `floor` half-moves); its tags
/// are set, and after the moves of another game a note says where it comes from.
fun beginMaiaGame(
    app: io.github.vrubelroman.chesspenguin.ChessPenguinApp,
    session: uniffi.mobile_api.GameSession,
    strength: Int,
    userWhite: Boolean,
    name: String,
    floor: Int,
    event: String,
    note: String?,
) {
    val date = java.text.SimpleDateFormat("yyyy.MM.dd", java.util.Locale.ROOT).format(java.util.Date())
    val tags = uniffi.mobile_api.maiaGameTags(event, userWhite, name, strength.toUInt()) + uniffi.mobile_api.Tag("Date", date)
    session.setData(tags, "*")
    if (floor > 0 && note != null) {
        val data = session.data()
        runCatching { session.addComment(lineEnd(data).toUInt(), note) }
    }
    session.markSaved()
    app.maiaGames.begin(session.pgn(), PlayMeta(strength, userWhite, name, floor))
}

/// Maia did not start (§31.3): her file again, or leave the game.
@Composable
fun FailureCard(onRetry: () -> Unit, onAbort: () -> Unit) {
    Card {
        Text(stringResource(R.string.play_maia_stopped), style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.error)
        Row(Modifier.fillMaxWidth().padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(onClick = onRetry, modifier = Modifier.weight(1f)) { Text(stringResource(R.string.play_maia_reload), maxLines = 1, overflow = TextOverflow.Ellipsis) }
            OutlinedButton(onClick = onAbort, modifier = Modifier.weight(1f)) { Text(stringResource(R.string.play_abort), maxLines = 1) }
        }
    }
}

/// A new game while one is put aside (§31.7): go on with that one, or begin anew (it is
/// replaced).
@Composable
fun AdjournedQuestion(meta: PlayMeta, onContinue: () -> Unit, onNew: () -> Unit, onDismiss: () -> Unit) {
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.play_adjourned_q, meta.strength.toString())) },
        text = { Text(stringResource(R.string.play_adjourned_new_text)) },
        confirmButton = { androidx.compose.material3.TextButton(onClick = onContinue) { Text(stringResource(R.string.play_continue_it)) } },
        dismissButton = { androidx.compose.material3.TextButton(onClick = onNew) { Text(stringResource(R.string.play_start_new)) } },
    )
}
