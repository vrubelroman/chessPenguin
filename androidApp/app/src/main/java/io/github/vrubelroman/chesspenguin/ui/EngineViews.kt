package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.ExpandLess
import androidx.compose.material.icons.rounded.ExpandMore
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.vrubelroman.chesspenguin.BuildConfig
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.EngineStatus
import io.github.vrubelroman.chesspenguin.data.NetProblem
import io.github.vrubelroman.chesspenguin.data.NetState
import io.github.vrubelroman.chesspenguin.theme.Tabular
import uniffi.mobile_api.Analysis
import uniffi.mobile_api.EngineLine
import kotlin.math.exp

/// White's share of the bar, as on Lichess: 0.5 is equal, 1 is White winning.
fun whiteShare(line: EngineLine?): Float {
    if (line == null) return 0.5f
    line.mate?.let { return if (it > 0) 1f else 0f }
    val cp = (line.cp ?: 0).coerceIn(-1000, 1000)
    return (0.5 + 0.5 * (2 / (1 + exp(-0.00368208 * cp)) - 1)).toFloat()
}

/// The engine's row between the board and the notation (android-ui.md §8.2): the
/// evaluation, the depth, the best line; ▾ shows more lines. While the game is being
/// analysed (`waiting`, §21.6) the engine rests and the row says so.
@Composable
fun EngineStrip(
    ending: String?,
    status: EngineStatus,
    analysis: Analysis?,
    flipped: Boolean,
    lines: Int,
    expanded: Boolean,
    onExpand: () -> Unit,
    onLine: (EngineLine) -> Unit,
    onDeeper: () -> Unit,
    onRestart: () -> Unit,
    notation: (String) -> AnnotatedString,
    waiting: Boolean = false,
) {
    val best = analysis?.lines?.firstOrNull()
    val share by animateFloatAsState(whiteShare(best), tween(300), label = "eval")
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainerLow)) {
        // White's share on the left; on the right when the board is turned.
        Row(Modifier.fillMaxWidth().height(3.dp)) {
            // The light part must stand out from a light strip too.
            val white = if (MaterialTheme.colorScheme.surface.luminance() > 0.5f) Color(0xFFD5D8DC) else Color(0xFFF5F5F0)
            val black = Color(0xFF26282B)
            val left = if (flipped) 1f - share else share
            if (left > 0f) Box(Modifier.weight(left.coerceAtLeast(0.001f)).fillMaxHeight().background(if (flipped) black else white))
            if (left < 1f) Box(Modifier.weight((1f - left).coerceAtLeast(0.001f)).fillMaxHeight().background(if (flipped) white else black))
        }
        if (ending != null) {
            Row(Modifier.fillMaxWidth().height(48.dp).padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(ending, style = MaterialTheme.typography.labelLarge)
            }
            return@Column
        }
        if (waiting) {
            Row(Modifier.fillMaxWidth().height(48.dp).padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(
                    stringResource(R.string.review_engine_paused),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            return@Column
        }
        if (status is EngineStatus.Failed) {
            Row(Modifier.fillMaxWidth().height(48.dp).padding(start = 16.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(
                    stringResource(if (status.notStarting) R.string.engine_not_starting else R.string.engine_not_answering),
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.weight(1f),
                )
                if (!status.notStarting) TextButton(onClick = onRestart) { Text(stringResource(R.string.restart)) }
            }
            return@Column
        }
        LineRow(best, analysis, status, true, lines > 1, expanded, onExpand, onLine, onDeeper, notation)
        if (expanded) {
            analysis?.lines?.drop(1)?.take(lines - 1)?.forEach { l ->
                LineRow(l, analysis, status, false, false, false, {}, onLine, {}, notation)
            }
        }
    }
}

@Composable
private fun LineRow(
    line: EngineLine?,
    analysis: Analysis?,
    status: EngineStatus,
    first: Boolean,
    canExpand: Boolean,
    expanded: Boolean,
    onExpand: () -> Unit,
    onLine: (EngineLine) -> Unit,
    onDeeper: () -> Unit,
    notation: (String) -> AnnotatedString,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .height(if (first) 48.dp else 40.dp)
            .then(if (line != null) Modifier.clickable { onLine(line) } else Modifier)
            .padding(start = 16.dp, end = if (first) 0.dp else 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        EvalPlaque(line)
        Spacer(Modifier.width(8.dp))
        if (first) {
            val depth = analysis?.depth?.toInt() ?: 0
            val label = when {
                line == null || status == EngineStatus.Starting -> stringResource(R.string.thinking)
                status == EngineStatus.Paused -> stringResource(R.string.depth, depth) + " · " + stringResource(R.string.paused)
                else -> stringResource(R.string.depth, depth)
            }
            Text(
                label,
                style = MaterialTheme.typography.bodyMedium.merge(Tabular),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
            )
            Spacer(Modifier.width(8.dp))
        }
        Text(
            line?.let { notation(it.text) } ?: AnnotatedString(""),
            style = MaterialTheme.typography.bodyMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        if (first && status == EngineStatus.Paused) {
            TextButton(onClick = onDeeper) { Text(stringResource(R.string.deeper)) }
        }
        if (first && canExpand) {
            IconButton(onClick = onExpand) {
                Icon(
                    if (expanded) Icons.Rounded.ExpandLess else Icons.Rounded.ExpandMore,
                    stringResource(if (expanded) R.string.fewer_lines else R.string.more_lines),
                )
            }
        }
    }
}

/// The evaluation: light when White is better, dark when Black is (§8.2).
@Composable
private fun EvalPlaque(line: EngineLine?) {
    // Dark only when Black is better by what is shown ("0.0" stays light).
    val blackBetter = line != null && ((line.mate ?: 0) < 0 || (line.mate == null && Math.round((line.cp ?: 0) / 10.0) < 0))
    val bg = if (blackBetter) Color(0xFF26282B) else Color(0xFFF5F5F0)
    val fg = if (blackBetter) Color(0xFFE8E8E8) else Color(0xFF1C1D1F)
    Box(
        Modifier
            .width(56.dp)
            .height(28.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(bg)
            .border(1.dp, MaterialTheme.colorScheme.outline, RoundedCornerShape(8.dp)),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            line?.let(::shownEval) ?: "",
            color = fg,
            fontSize = 15.sp,
            fontWeight = FontWeight.Bold,
            style = Tabular,
        )
    }
}

/// The evaluation as shown (§8.2): pawns with one decimal, rounded ("+0.45" → "+0.5"),
/// no sign for an equal position; mates as the engine line has them ("#3", "#−2").
fun shownEval(line: EngineLine): String {
    if (line.mate != null) return line.eval
    val tenths = Math.round((line.cp ?: 0) / 10.0).toInt()
    if (tenths == 0) return "0.0"
    val sign = if (tenths > 0) "+" else "\u2212"
    val a = kotlin.math.abs(tenths)
    return "$sign${a / 10}.${a % 10}"
}

/// The bar above the notation while an engine line is shown on the board (§8.3); `best`
/// for the better line of a game report's error (§21.1).
@Composable
fun PreviewBar(step: Int, total: Int, onClose: () -> Unit, best: Boolean = false) {
    Row(
        Modifier
            .fillMaxWidth()
            .height(48.dp)
            .background(MaterialTheme.colorScheme.primaryContainer)
            .padding(start = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            stringResource(if (best) R.string.review_best_preview else R.string.engine_line_preview, step, total),
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.onPrimaryContainer,
            modifier = Modifier.weight(1f),
        )
        IconButton(onClick = onClose) { Icon(Icons.Rounded.Close, stringResource(R.string.close)) }
    }
}

private fun mb(bytes: Long) = (bytes / 1_000_000).toInt()

/// The sheet that offers the download of the engine (light build, §8.4).
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun EngineSheet(state: NetState, metered: Boolean, onDownload: () -> Unit, onCancel: () -> Unit, onDismiss: () -> Unit) {
    val size = mb(BuildConfig.NET_SIZE)
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, bottom = 24.dp)) {
            Text(stringResource(R.string.engine_sheet_title), style = MaterialTheme.typography.titleLarge)
            Spacer(Modifier.height(16.dp))
            Text(stringResource(R.string.engine_sheet_text), style = MaterialTheme.typography.bodyLarge)
            Spacer(Modifier.height(16.dp))
            when (state) {
                is NetState.Loading -> {
                    LinearProgressIndicator(
                        progress = { state.done.toFloat() / state.total.coerceAtLeast(1) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(Modifier.height(8.dp))
                    Text(
                        stringResource(R.string.downloaded_of, mb(state.done), mb(state.total)),
                        style = MaterialTheme.typography.bodyMedium.merge(Tabular),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.height(24.dp))
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                        TextButton(onClick = onCancel) { Text(stringResource(R.string.cancel_download)) }
                    }
                }
                is NetState.Failed -> {
                    val text = when (state.problem) {
                        NetProblem.NoInternet -> stringResource(R.string.no_internet)
                        NetProblem.Interrupted -> stringResource(R.string.interrupted)
                        NetProblem.NoSpace -> stringResource(R.string.no_space, size)
                        NetProblem.Corrupt -> stringResource(R.string.corrupt)
                    }
                    Text(text, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
                    Spacer(Modifier.height(24.dp))
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                        if (state.problem != NetProblem.NoSpace) TextButton(onClick = onDismiss) { Text(stringResource(R.string.not_now)) }
                        when (state.problem) {
                            NetProblem.NoSpace -> Button(onClick = onDismiss) { Text(stringResource(R.string.ok)) }
                            NetProblem.NoInternet, NetProblem.Interrupted -> Button(onClick = onDownload) { Text(stringResource(R.string.retry)) }
                            NetProblem.Corrupt -> Button(onClick = onDownload) { Text(stringResource(R.string.download_again)) }
                        }
                    }
                }
                else -> {
                    Text(stringResource(R.string.engine_size, size), style = MaterialTheme.typography.bodyMedium.merge(Tabular))
                    if (metered) {
                        Spacer(Modifier.height(8.dp))
                        Text(
                            stringResource(R.string.engine_metered),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    Spacer(Modifier.height(24.dp))
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                        TextButton(onClick = onDismiss) { Text(stringResource(R.string.not_now)) }
                        Button(onClick = onDownload) { Text(stringResource(R.string.download)) }
                    }
                }
            }
        }
    }
}
