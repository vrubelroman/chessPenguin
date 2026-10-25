package io.github.vrubelroman.chesspenguin.ui

import android.view.HapticFeedbackConstants
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.rounded.Replay
import androidx.compose.material.icons.rounded.SwapVert
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.vrubelroman.chesspenguin.R
import kotlinx.coroutines.delay
import uniffi.mobile_api.TrainVerdict
import uniffi.mobile_api.Trainer

/// A wrong move of the trainer, on the board for a moment (§32.2).
private data class Wrong(val fen: String, val from: String, val to: String)

/// Learning an opening (android-play.md §32): the user plays the moves of both sides
/// of the line from memory; a wrong move shows for 400 ms and is taken back.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TrainScreen(trainer: Trainer, ply0: Int, startFlipped: Boolean, onPly: (Int) -> Unit, onExit: () -> Unit) {
    val app = app()
    val prefs by app.settings.prefs.collectAsState()
    val context = androidx.compose.ui.platform.LocalContext.current
    val pieces = remember { Pieces(context) }
    val view = LocalView.current
    val lang = uiLanguage()
    val san = remember(prefs.notation, lang) { { s: String -> displaySan(s, prefs.notation, lang) } }
    var ply by remember { mutableIntStateOf(ply0) }
    var flipped by remember { mutableStateOf(startFlipped) }
    var wrong by remember { mutableStateOf<Wrong?>(null) }
    // Said after a wrong move until the right one.
    var missed by remember { mutableStateOf(false) }
    var changes by remember { mutableIntStateOf(0) }
    val st = remember(ply, changes) { trainer.state(ply.toUInt()) }
    BackHandler(onBack = onExit)
    LaunchedEffect(wrong) {
        if (wrong != null) {
            delay(400)
            wrong = null
        }
    }
    fun buzz(reject: Boolean) {
        if (prefs.vibration) view.performHapticFeedback(if (reject) HapticFeedbackConstants.REJECT else HapticFeedbackConstants.CLOCK_TICK)
    }
    fun move(uci: String) {
        when (trainer.judge(ply.toUInt(), uci)) {
            TrainVerdict.RIGHT -> {
                missed = false
                ply += 1
                onPly(ply)
                buzz(false)
                trainer.state(ply.toUInt()).played.lastOrNull()?.let { app.sounds.move(it) }
            }
            TrainVerdict.FINISHED -> {
                missed = false
                ply += 1
                onPly(ply)
                buzz(false)
                app.sounds.play(io.github.vrubelroman.chesspenguin.data.Sound.Success)
            }
            TrainVerdict.WRONG -> {
                trainer.fenAfter(ply.toUInt(), uci)?.let { wrong = Wrong(it, uci.substring(0, 2), uci.substring(2, 4)) }
                missed = true
                changes++
                buzz(true)
                app.sounds.play(io.github.vrubelroman.chesspenguin.data.Sound.Wrong)
            }
            TrainVerdict.DONE -> {}
        }
    }
    val sub = listOfNotNull(trainer.game(), stringResource(R.string.train_up_to, san(trainer.upTo()).text)).joinToString(" · ")
    Scaffold(
        topBar = {
            TopAppBar(
                navigationIcon = { IconButton(onClick = onExit) { Icon(Icons.AutoMirrored.Rounded.ArrowBack, stringResource(R.string.back)) } },
                title = {
                    Column {
                        Text(stringResource(R.string.train_title), style = MaterialTheme.typography.titleMedium)
                        Text(sub, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                },
            )
        },
        bottomBar = {
            Column {
                HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
                    Row(Modifier.fillMaxWidth().navigationBarsPadding().height(72.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.SpaceEvenly) {
                        IconButton(onClick = { flipped = !flipped }) { Icon(Icons.Rounded.SwapVert, stringResource(R.string.flip)) }
                        TextButton(enabled = ply > 0 || st.done, onClick = {
                            trainer.again()
                            ply = 0
                            onPly(0)
                            missed = false
                            wrong = null
                            changes++
                        }) {
                            Icon(Icons.Rounded.Replay, null)
                            Spacer(Modifier.width(8.dp))
                            Text(stringResource(R.string.train_restart))
                        }
                    }
                }
            }
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            if (!st.done) {
                // The task: which move, whose turn, how far.
                val whiteToMove = st.fen.substringAfter(' ').startsWith("w")
                Column(Modifier.fillMaxWidth().height(48.dp).padding(horizontal = 16.dp), verticalArrangement = Arrangement.Center) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(stringResource(R.string.train_move_of, (ply + 1).toString(), st.length.toString()), Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
                        Text(stringResource(if (whiteToMove) R.string.train_white else R.string.train_black), style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Medium)
                    }
                    LinearProgressIndicator(progress = { ply.toFloat() / st.length.toInt().coerceAtLeast(1) }, modifier = Modifier.fillMaxWidth().padding(top = 4.dp))
                }
            }
            val w = wrong
            Board(
                fen = w?.fen ?: st.fen,
                flipped = flipped,
                lastFrom = w?.from ?: st.fromSquare,
                lastTo = w?.to ?: st.toSquare,
                pieces = pieces,
                theme = prefs.board,
                set = prefs.pieces,
                description = stringResource(R.string.train_title),
                arrows = st.hint?.let { listOf(BoardArrow(it[0], it[1], EngineArrow)) } ?: emptyList(),
                animated = prefs.animation,
                modifier = Modifier.fillMaxWidth(),
                input = if (st.done || w != null) null else BoardInput(
                    remember(ply, changes) { trainer.legalMoves(ply.toUInt()) },
                    onMove = ::move,
                    // The line's own piece where it promotes; a queen otherwise (§29.4).
                    promotion = { pair -> trainer.promotionAt(ply.toUInt())?.takeIf { it.startsWith(pair) }?.last() },
                ),
                alert = w?.let { it.from to it.to },
            )
            if (!st.done) {
                // You: the mistakes of this run, or before the first move what was before.
                Row(Modifier.fillMaxWidth().height(48.dp).padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    val text = when {
                        st.hint != null -> stringResource(R.string.train_arrow)
                        missed -> stringResource(R.string.train_wrong)
                        ply == 0 && st.mistakes == 0u -> history(st.before)
                        else -> stringResource(R.string.train_mistakes, st.mistakes.toString())
                    }
                    Text(
                        text,
                        Modifier.weight(1f),
                        color = if (missed && st.hint == null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (st.mayShow && st.hint == null) {
                        TextButton(onClick = {
                            trainer.show(ply.toUInt())
                            changes++
                        }) { Text(stringResource(R.string.train_show)) }
                    } else if (ply > 0 || st.mistakes > 0u) {
                        // With the message of a wrong move, the count stays in sight.
                        Text(
                            if (missed && st.hint == null) stringResource(R.string.train_mistakes, st.mistakes.toString())
                            else stringResource(R.string.train_run, (st.before.runs + 1u).toString()),
                            Modifier.padding(end = 12.dp),
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            style = MaterialTheme.typography.bodyMedium,
                        )
                    }
                }
            }
            Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState())) {
                if (st.done) Result(st, onAgain = {
                    trainer.again()
                    ply = 0
                    onPly(0)
                    missed = false
                    changes++
                }, onExit = onExit)
                // The moves played so far, two to a row.
                st.played.chunked(2).forEachIndexed { i, pair ->
                    Row(Modifier.fillMaxWidth().height(40.dp).padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text("${i + 1}", Modifier.width(32.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Text(san(pair[0]), Modifier.weight(1f), fontWeight = FontWeight.Medium)
                        Text(pair.getOrNull(1)?.let { san(it) } ?: androidx.compose.ui.text.AnnotatedString(""), Modifier.weight(1f), fontWeight = FontWeight.Medium)
                    }
                }
            }
        }
    }
}

/// «Last time: 2 · best: 0», «Not done before».
@Composable
private fun history(s: uniffi.mobile_api.TrainStats): String = when {
    s.runs == 0u -> stringResource(R.string.train_not_done)
    s.cleanStreak >= 2u -> stringResource(R.string.train_last_streak, (s.last ?: 0u).toString(), s.cleanStreak.toString())
    else -> stringResource(R.string.train_last_best, (s.last ?: 0u).toString(), s.best?.toString() ?: "–")
}

/// The end of a run (§32.3): how it went, the line's runs, again or back to the game.
@Composable
private fun Result(st: uniffi.mobile_api.TrainState, onAgain: () -> Unit, onExit: () -> Unit) {
    val after = st.after ?: st.before
    Column(
        Modifier
            .fillMaxWidth()
            .padding(16.dp)
            .background(MaterialTheme.colorScheme.surfaceContainerHigh, RoundedCornerShape(16.dp))
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text(
            when {
                st.hinted -> stringResource(R.string.train_done_hint, st.mistakes.toString())
                st.mistakes == 0u -> stringResource(R.string.train_done_clean)
                else -> stringResource(R.string.train_done, st.mistakes.toString())
            },
            style = MaterialTheme.typography.titleMedium,
        )
        val line = buildList {
            add(stringResource(R.string.train_run_best, after.runs.toString(), after.best?.toString() ?: "–"))
            if (after.cleanStreak >= 2u) add(stringResource(R.string.train_streak, after.cleanStreak.toString()))
        }.joinToString(" · ")
        Text(line, style = MaterialTheme.typography.bodyMedium)
        if (after.recentMistakes.size > 1) {
            val runs = after.recentMistakes.zip(after.recentHinted).joinToString(" · ") { (m, h) -> if (h) "$m*" else "$m" }
            Text(stringResource(R.string.train_recent, runs), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Row(Modifier.fillMaxWidth().padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            // The desktop's texts carry their signs; here the buttons are the signs.
            Button(onClick = onAgain, modifier = Modifier.weight(1f).height(48.dp)) { Text(stringResource(R.string.train_again).removePrefix("↻ "), maxLines = 1) }
            OutlinedButton(onClick = onExit, modifier = Modifier.weight(1f).height(48.dp)) { Text(stringResource(R.string.train_back).removePrefix("⬅ "), maxLines = 1) }
        }
    }
}
