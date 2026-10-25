package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.NetProblem
import io.github.vrubelroman.chesspenguin.data.NetState
import io.github.vrubelroman.chesspenguin.theme.Tabular
import uniffi.mobile_api.maiaRange

/// The position a game against Maia starts from (android-play.md §31.6): drawn small,
/// with where it comes from; the side to move plays by default.
data class PlayFrom(val fen: String, val label: String, val whiteToMove: Boolean)

/// «Play against the computer» (android-play.md §31.1): Maia's strength, the colour,
/// the name; Maia downloaded here the first time (21 MB). `onPlay`: strength, the user
/// plays White, name.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PlaySheet(from: PlayFrom?, onPlay: (Int, Boolean, String) -> Unit, onDismiss: () -> Unit) {
    val app = app()
    val prefs by app.settings.prefs.collectAsState()
    val files = app.maiaFiles.download
    val state by files.state.collectAsState()
    val range = remember { maiaRange() }
    val lo = range[0].toInt()
    val hi = range[1].toInt()
    var strength by remember { mutableFloatStateOf(prefs.maiaStrength.coerceIn(lo, hi).toFloat()) }
    // From a position: the side to move by default, not remembered (play-from-position.md §2).
    var color by remember { mutableIntStateOf(if (from != null) (if (from.whiteToMove) 1 else -1) else prefs.maiaColor) }
    var name by remember { mutableStateOf(prefs.yourName) }
    // «Download and play»: the game starts once the file is checked.
    var playAfter by remember { mutableStateOf(false) }
    val level = (strength / 50f).toInt() * 50
    fun play() {
        val white = when (color) {
            1 -> true
            -1 -> false
            else -> kotlin.random.Random.nextBoolean()
        }
        app.settings.update { p ->
            p.copy(maiaStrength = level, yourName = name.trim(), maiaColor = if (from != null) p.maiaColor else color)
        }
        onPlay(level, white, name.trim())
    }
    LaunchedEffect(state) {
        if (playAfter && state == NetState.Ready) {
            playAfter = false
            play()
        }
    }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(start = 24.dp, end = 24.dp, bottom = 24.dp)) {
            Text(stringResource(R.string.play_title), style = MaterialTheme.typography.titleLarge)
            if (from != null) {
                Row(Modifier.padding(top = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                    MiniBoard(from.fen, flipped = color == -1, size = 96.dp)
                    Text(from.label, Modifier.padding(start = 16.dp), style = MaterialTheme.typography.bodyMedium, maxLines = 4, overflow = TextOverflow.Ellipsis)
                }
            }
            Spacer(Modifier.height(16.dp))
            Text(stringResource(R.string.play_opponent, level.toString()), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Medium)
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("$lo", style = MaterialTheme.typography.bodySmall.merge(Tabular), color = MaterialTheme.colorScheme.onSurfaceVariant)
                Slider(
                    value = strength,
                    onValueChange = { strength = it },
                    valueRange = lo.toFloat()..hi.toFloat(),
                    modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
                )
                Text("$hi", style = MaterialTheme.typography.bodySmall.merge(Tabular), color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Text(stringResource(R.string.play_measured), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(16.dp))
            Text(stringResource(R.string.play_colour), style = MaterialTheme.typography.titleSmall)
            Spacer(Modifier.height(8.dp))
            val options = listOf(1 to R.string.color_white, 0 to R.string.color_random, -1 to R.string.color_black)
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                options.forEachIndexed { i, (v, label) ->
                    SegmentedButton(selected = color == v, onClick = { color = v }, shape = SegmentedButtonDefaults.itemShape(i, options.size)) {
                        Text(stringResource(label))
                    }
                }
            }
            Spacer(Modifier.height(12.dp))
            OutlinedTextField(name, { name = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(R.string.play_your_name)) }, singleLine = true)
            Spacer(Modifier.height(12.dp))
            // The state of Maia's file: always one line, so that nothing jumps.
            Row(Modifier.fillMaxWidth().height(40.dp), verticalAlignment = Alignment.CenterVertically) {
                when (val s = state) {
                    NetState.Ready -> Text(stringResource(R.string.play_maia_ready), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    NetState.Missing -> Text(
                        stringResource(R.string.play_download_note, megabytes(files.size)),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 2,
                    )
                    is NetState.Loading -> {
                        Column(Modifier.weight(1f)) {
                            LinearProgressIndicator(progress = { s.done.toFloat() / s.total.coerceAtLeast(1) }, modifier = Modifier.fillMaxWidth())
                            Text(stringResource(R.string.downloaded_of, megabytes(s.done), megabytes(s.total)), style = MaterialTheme.typography.bodySmall.merge(Tabular))
                        }
                        IconButton(onClick = {
                            playAfter = false
                            files.cancel()
                        }) { Icon(Icons.Rounded.Close, stringResource(R.string.cancel_download)) }
                    }
                    is NetState.Failed -> Text(
                        when (s.problem) {
                            NetProblem.NoInternet -> stringResource(R.string.no_internet)
                            NetProblem.Interrupted -> stringResource(R.string.interrupted)
                            NetProblem.NoSpace -> stringResource(R.string.no_space, megabytes(files.size))
                            NetProblem.Corrupt -> stringResource(R.string.corrupt)
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                        maxLines = 2,
                    )
                }
            }
            Spacer(Modifier.height(8.dp))
            val loading = state is NetState.Loading
            Button(
                onClick = {
                    if (state == NetState.Ready) {
                        play()
                    } else {
                        playAfter = true
                        files.download()
                    }
                },
                enabled = !loading,
                modifier = Modifier.fillMaxWidth().height(56.dp),
            ) {
                Text(
                    stringResource(
                        when (state) {
                            NetState.Ready, is NetState.Loading -> R.string.play_play
                            is NetState.Failed -> R.string.play_retry
                            NetState.Missing -> R.string.play_download_and_play
                        },
                    ),
                )
            }
        }
    }
}

/// A small board of a position (no touches, no coordinates).
@Composable
fun MiniBoard(fen: String, flipped: Boolean, size: androidx.compose.ui.unit.Dp) {
    val app = app()
    val prefs by app.settings.prefs.collectAsState()
    val context = androidx.compose.ui.platform.LocalContext.current
    val pieces = remember { Pieces(context) }
    Board(
        fen = fen,
        flipped = flipped,
        lastFrom = null,
        lastTo = null,
        pieces = pieces,
        theme = prefs.board,
        set = prefs.pieces,
        description = "",
        animated = false,
        modifier = Modifier.width(size),
    )
}

private fun megabytes(bytes: Long) = ((bytes + 999_999) / 1_000_000).toInt()
