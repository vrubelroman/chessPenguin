package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.ChevronRight
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.GUESS_NPS
import io.github.vrubelroman.chesspenguin.theme.LocalChessColors
import io.github.vrubelroman.chesspenguin.theme.Tabular
import io.github.vrubelroman.chesspenguin.theme.nagColor
import uniffi.mobile_api.GameData
import uniffi.mobile_api.MoveJudgement
import uniffi.mobile_api.ReviewMove
import uniffi.mobile_api.ReviewState
import uniffi.mobile_api.SideReview
import kotlin.math.roundToInt

/// The NAG of the same sign: $3 !!, $1 !, $6 ?!, $2 ?, $4 ??.
fun judgementNag(j: MoveJudgement): Int = when (j) {
    MoveJudgement.INACCURACY -> 6
    MoveJudgement.MISTAKE -> 2
    MoveJudgement.BLUNDER -> 4
    MoveJudgement.GREAT -> 1
    MoveJudgement.BRILLIANT -> 3
}

/// The colors of the NAGs !!, !, ?!, ? and ?? (§21.4).
fun judgementColor(j: MoveJudgement, dark: Boolean): Color = nagColor(judgementNag(j), dark) ?: Color.Unspecified

/// The sign of a move assessment NAG ($1–$6), `null` for others.
fun assessmentSymbol(nag: Int): String? = when (nag) {
    1 -> "!"
    2 -> "?"
    3 -> "!!"
    4 -> "??"
    5 -> "!?"
    6 -> "?!"
    else -> null
}

fun judgementSymbol(j: MoveJudgement): String = when (j) {
    MoveJudgement.INACCURACY -> "?!"
    MoveJudgement.MISTAKE -> "?"
    MoveJudgement.BLUNDER -> "??"
    MoveJudgement.GREAT -> "!"
    MoveJudgement.BRILLIANT -> "!!"
}

@Composable
fun judgementName(j: MoveJudgement): String = stringResource(
    when (j) {
        MoveJudgement.INACCURACY -> R.string.review_inaccuracy
        MoveJudgement.MISTAKE -> R.string.review_mistake
        MoveJudgement.BLUNDER -> R.string.review_blunder
        MoveJudgement.GREAT -> R.string.review_great
        MoveJudgement.BRILLIANT -> R.string.review_brilliant
    },
)

@Composable
private fun judgementPlural(j: MoveJudgement): String = stringResource(
    when (j) {
        MoveJudgement.INACCURACY -> R.string.review_inaccuracies
        MoveJudgement.MISTAKE -> R.string.review_mistakes
        MoveJudgement.BLUNDER -> R.string.review_blunders
        MoveJudgement.GREAT -> R.string.review_greats
        MoveJudgement.BRILLIANT -> R.string.review_brilliants
    },
)

/// The place in `state.points` of a node: itself on the main line, or the main-line move
/// its variation leaves from.
fun pointIndex(points: Map<Int, Int>, game: GameData, node: Int): Int? {
    var n = node
    while (true) {
        points[n]?.let { return it }
        n = game.nodes.getOrNull(n)?.parent?.toInt() ?: return null
    }
}

/// "91%", or "—" when the accuracy is not known yet.
private fun percent(a: Double?): String = a?.let { "${it.roundToInt()}%" } ?: "—"

/// The analysis between the board and the notation (§21.3): what is going on or what the
/// current move was, and the evaluation graph.
@Composable
fun ReviewPanel(
    state: ReviewState,
    cursor: Int?,
    mark: ReviewMove?,
    preparing: Boolean,
    notation: (String) -> AnnotatedString,
    onIndex: (Int) -> Unit,
    onBest: (ReviewMove) -> Unit,
    onReport: () -> Unit,
    onStop: () -> Unit,
    onResume: () -> Unit,
    cursorLabel: String? = null,
    showGraph: Boolean = true,
) {
    val dark = LocalChessColors.current.dark
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainerLow)) {
        // 48 dp: every button here and the error row itself are touch targets (§21.3).
        Row(
            Modifier.fillMaxWidth().height(48.dp).padding(start = 16.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val textStyle = MaterialTheme.typography.bodyMedium
            val report: @Composable () -> Unit = {
                TextButton(onClick = onReport) { Text(stringResource(R.string.review_report)) }
            }
            when {
                state.running -> {
                    Text(
                        stringResource(R.string.review_progress, state.done.toInt(), state.total.toInt()),
                        style = textStyle.merge(Tabular),
                        modifier = Modifier.weight(1f),
                    )
                    IconButton(onClick = onStop) { Icon(Icons.Rounded.Close, stringResource(R.string.review_stop)) }
                }
                preparing -> Text(
                    stringResource(R.string.preparing_engine),
                    style = textStyle,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.weight(1f),
                )
                mark != null -> {
                    val c = judgementColor(mark.judgement, dark)
                    val name = judgementName(mark.judgement)
                    val canShow = mark.best?.steps?.isNotEmpty() == true
                    // Without a line (evaluations from the PGN) the better move is not named.
                    val better = mark.bestLabel.takeIf { it.isNotEmpty() && canShow }?.let { notation(it) }
                    // The template around the move ("{move} was better", "лучше было {move}").
                    val betterText = better?.let { stringResource(R.string.review_better, "\u0000").split('\u0000') }
                    val link = MaterialTheme.colorScheme.primary
                    val text = buildAnnotatedString {
                        withStyle(SpanStyle(color = c, fontWeight = FontWeight.Bold)) { append(judgementSymbol(mark.judgement) + " " + name) }
                        if (better != null && betterText != null) {
                            append(" · ")
                            append(betterText.first())
                            // The move in the link color: the row can be tapped (§21.3).
                            withStyle(SpanStyle(color = link, fontWeight = FontWeight.Medium)) { append(better) }
                            append(betterText.getOrElse(1) { "" })
                        }
                    }
                    Box(
                        Modifier
                            .weight(1f)
                            .height(48.dp)
                            .then(if (canShow) Modifier.clickable(onClickLabel = stringResource(R.string.review_show_best)) { onBest(mark) } else Modifier),
                        contentAlignment = Alignment.CenterStart,
                    ) {
                        Text(text, style = textStyle, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    report()
                }
                state.failure != null -> {
                    Text(stringResource(R.string.review_failed), style = textStyle, color = MaterialTheme.colorScheme.error, modifier = Modifier.weight(1f))
                    TextButton(onClick = onResume) { Text(stringResource(R.string.review_resume)) }
                    report()
                }
                // Made with other engine settings (before the fixed search): made again.
                state.stale -> {
                    Text(
                        stringResource(R.string.review_stale),
                        style = textStyle,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    TextButton(onClick = onResume) { Text(stringResource(R.string.review_update)) }
                    report()
                }
                state.missing > 0u || state.secondsMissing > 0u -> {
                    Text(
                        // An analysis from before !! and !: filling in looks for them only.
                        if (state.missing > 0u) stringResource(R.string.review_missing, state.missing.toInt()) else stringResource(R.string.review_no_seconds),
                        style = textStyle.merge(Tabular),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.weight(1f),
                    )
                    TextButton(onClick = onResume) { Text(stringResource(R.string.review_fill)) }
                    report()
                }
                else -> {
                    val spoken = stringResource(R.string.review_accuracy_of, percent(state.white.accuracy), percent(state.black.accuracy))
                    Row(
                        Modifier.weight(1f).clearAndSetSemantics { contentDescription = spoken },
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(stringResource(R.string.review_accuracy), style = textStyle, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Spacer(Modifier.width(10.dp))
                        SideDot(white = true)
                        Text(percent(state.white.accuracy), style = textStyle.merge(Tabular), fontWeight = FontWeight.Medium, modifier = Modifier.padding(start = 6.dp))
                        Spacer(Modifier.width(14.dp))
                        SideDot(white = false)
                        Text(percent(state.black.accuracy), style = textStyle.merge(Tabular), fontWeight = FontWeight.Medium, modifier = Modifier.padding(start = 6.dp))
                    }
                    report()
                }
            }
        }
        if (showGraph) {
            EvalGraph(state, cursor, cursorLabel, onIndex, Modifier.fillMaxWidth().height(40.dp).padding(start = 16.dp, end = 16.dp, bottom = 8.dp))
        }
    }
}

/// A circle in a side's color, outlined so the white one shows on a light background.
@Composable
private fun SideDot(white: Boolean, size: Int = 12) {
    Box(
        Modifier
            .size(size.dp)
            .clip(CircleShape)
            .background(if (white) Color(0xFFF5F5F0) else Color(0xFF26282B))
            .border(1.dp, MaterialTheme.colorScheme.outline, CircleShape),
    )
}

/// White's chance of winning along the main line (§21.3), as on the desktop: light area
/// for White, dark for Black, the curve, the errors as dots, the phases and the current
/// move. A tap or a drag goes to a move.
@Composable
fun EvalGraph(state: ReviewState, cursor: Int?, cursorLabel: String?, onIndex: (Int) -> Unit, modifier: Modifier) {
    val chess = LocalChessColors.current
    val dark = chess.dark
    // In the light theme a shade darker than the panel, so that White's area shows.
    val bg = if (dark) MaterialTheme.colorScheme.surfaceContainerHighest else Color(0xFFDCDFE3)
    val whiteFill = if (dark) Color(0xFF969696) else Color(0xFFFFFFFF)
    val blackFill = if (dark) Color(0xFF0A0A0A) else Color(0xFF464646)
    val curve = Color(0xFFE07B1F)
    val midColor = MaterialTheme.colorScheme.outline
    val phaseColor = MaterialTheme.colorScheme.onSurfaceVariant
    val description = stringResource(R.string.review_graph)
    val now = cursor?.let { state.points.getOrNull(it) }
    val spokenState = now?.eval?.takeIf { it.isNotEmpty() }?.let { e ->
        stringResource(R.string.review_graph_state, cursorLabel ?: stringResource(R.string.review_start_position), e)
    }
    val points = state.points
    val n = points.size
    val pick by rememberUpdatedState(onIndex)
    Canvas(
        modifier
            .clip(RoundedCornerShape(6.dp))
            .background(bg)
            .semantics {
                contentDescription = description
                spokenState?.let { stateDescription = it }
            }
            .pointerInput(n) {
                if (n < 2) return@pointerInput
                val pad = 6.dp.toPx()
                fun index(x: Float): Int {
                    val t = ((x - pad) / (size.width - 2 * pad)).coerceIn(0f, 1f)
                    return (t * (n - 1)).roundToInt()
                }
                awaitEachGesture {
                    val down = awaitFirstDown()
                    var last = index(down.position.x)
                    pick(last)
                    down.consume()
                    while (true) {
                        val event = awaitPointerEvent()
                        val change = event.changes.firstOrNull { it.id == down.id } ?: break
                        if (!change.pressed) break
                        val i = index(change.position.x)
                        if (i != last) {
                            last = i
                            pick(i)
                        }
                        change.consume()
                    }
                }
            },
    ) {
        if (n < 2) return@Canvas
        val padX = 6.dp.toPx()
        val padY = 4.dp.toPx()
        val w = size.width - 2 * padX
        val h = size.height - 2 * padY
        fun x(i: Int) = padX + w * i / (n - 1)
        fun y(wp: Float) = padY + h * (1f - wp / 100f)
        val mid = y(50f)
        // Advantage areas between the curve and the middle line: one outline per run of
        // evaluated positions, drawn light above the middle and dark below it (one shape,
        // so no seams between the moves).
        val area = Path()
        var open = false
        var lastX = 0f
        points.forEachIndexed { i, p ->
            val wp = p.whiteWin
            if (wp == null) {
                if (open) {
                    area.lineTo(lastX, mid)
                    area.close()
                    open = false
                }
            } else {
                if (!open) {
                    area.moveTo(x(i), mid)
                    open = true
                }
                area.lineTo(x(i), y(wp))
                lastX = x(i)
            }
        }
        if (open) {
            area.lineTo(lastX, mid)
            area.close()
        }
        clipRect(bottom = mid) { drawPath(area, whiteFill) }
        clipRect(top = mid) { drawPath(area, blackFill) }
        drawLine(midColor.copy(alpha = 0.6f), Offset(padX, mid), Offset(size.width - padX, mid), 1.dp.toPx())
        // Phase boundaries, without labels: at this height they would cover the curve
        // (§21.3); the report has the accuracy per phase.
        listOfNotNull(state.middlegame?.toInt(), state.endgame?.toInt()).filter { it in 1 until n }.forEach { i ->
            drawLine(phaseColor.copy(alpha = 0.5f), Offset(x(i), 0f), Offset(x(i), size.height), 1.dp.toPx())
        }
        // The curve, broken where positions are not evaluated yet.
        val stroke = Stroke(width = 1.6.dp.toPx())
        var path: Path? = null
        var count = 0
        fun flush() {
            if (count > 1) path?.let { drawPath(it, curve, style = stroke) }
            path = null
            count = 0
        }
        points.forEachIndexed { i, p ->
            val wp = p.whiteWin
            if (wp == null) {
                flush()
            } else {
                val px = x(i)
                val py = y(wp)
                val current = path
                if (current == null) {
                    path = Path().apply { moveTo(px, py) }
                } else {
                    current.lineTo(px, py)
                }
                count++
            }
        }
        flush()
        // The errors on top of everything but the current move.
        points.forEachIndexed { i, p ->
            val j = p.judgement ?: return@forEachIndexed
            val wp = p.whiteWin ?: return@forEachIndexed
            drawCircle(judgementColor(j, dark), 3.5.dp.toPx(), Offset(x(i), y(wp)))
        }
        if (cursor != null && cursor in 0 until n) {
            drawLine(curve, Offset(x(cursor), 0f), Offset(x(cursor), size.height), 1.5.dp.toPx())
        }
    }
}

/// The sheet that starts the analysis (§21.2): what it does and how long it takes. The
/// engine search is fixed, the same on every phone and computer: nothing to choose.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ReviewStartSheet(
    state: ReviewState?,
    npsPerProcess: Long,
    onStart: (full: Boolean) -> Unit,
    onDismiss: () -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(start = 24.dp, end = 24.dp, bottom = 24.dp)) {
            Text(stringResource(R.string.review_menu), style = MaterialTheme.typography.titleLarge)
            Spacer(Modifier.height(12.dp))
            Text(stringResource(R.string.review_text), style = MaterialTheme.typography.bodyLarge)
            Spacer(Modifier.height(12.dp))
            val total = state?.positions?.toInt() ?: 0
            val missing = state?.missing?.toInt() ?: total
            // Evaluations from the PGN: the engine makes its own, all of them.
            val pgn = state != null && state.hasData && state.nodes == 0UL
            // Some positions already have an engine evaluation of today's search: filled in.
            val partial = state != null && state.hasData && !pgn && !state.stale && missing in 1 until total
            val positions = if (partial) missing else total
            // A full run also searches again for moves that could be !! or ! (about a third more).
            val searches = if (partial) positions else (state?.searches?.toInt() ?: positions)
            val secs = uniffi.mobile_api.reviewEstimateSecs(searches.toUInt(), (if (npsPerProcess > 0) npsPerProcess else GUESS_NPS).toULong()).toInt().coerceAtLeast(1)
            val time = if (secs < 60) stringResource(R.string.review_seconds, secs) else stringResource(R.string.review_minutes, (secs + 59) / 60)
            Text(
                pluralStringResource(R.plurals.review_estimate, positions, positions, time),
                style = MaterialTheme.typography.bodyMedium.merge(Tabular),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (partial) {
                Spacer(Modifier.height(4.dp))
                Text(
                    stringResource(R.string.review_partial, total - missing, total),
                    style = MaterialTheme.typography.bodyMedium.merge(Tabular),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Spacer(Modifier.height(24.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) }
                // The same search again would give the same evaluations: only the missing
                // ones are made, or all of them over the PGN's (or a stale analysis).
                Button(onClick = { onStart(pgn) }) { Text(stringResource(if (partial) R.string.review_fill else R.string.review_start)) }
            }
        }
    }
}

/// The report (§21.5): White and Black side by side, as on Lichess; a count above zero
/// goes to the next such move of that player.
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun ReviewSheet(
    state: ReviewState,
    white: String,
    black: String,
    onError: (white: Boolean, MoveJudgement) -> Unit,
    onAgain: () -> Unit,
    onHide: () -> Unit,
    onDismiss: () -> Unit,
) {
    // Opened whole: nothing in it should hide below the edge (§21.5).
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).navigationBarsPadding().padding(start = 24.dp, end = 16.dp, bottom = 24.dp)) {
            Text(stringResource(R.string.review_report_menu), style = MaterialTheme.typography.titleLarge)
            Spacer(Modifier.height(16.dp))
            val dark = LocalChessColors.current.dark
            val variant = MaterialTheme.colorScheme.onSurfaceVariant
            // Player names over the two columns.
            ReportRow(
                label = {},
                white = { PlayerHead(white, true) },
                black = { PlayerHead(black, false) },
                height = null,
            )
            ReportRow(
                label = { Text(stringResource(R.string.review_accuracy), style = MaterialTheme.typography.bodyLarge) },
                white = { ReportNumber(percent(state.white.accuracy), MaterialTheme.typography.headlineSmall) },
                black = { ReportNumber(percent(state.black.accuracy), MaterialTheme.typography.headlineSmall) },
            )
            val next = stringResource(R.string.review_next)
            listOf(
                MoveJudgement.BRILLIANT,
                MoveJudgement.GREAT,
                MoveJudgement.INACCURACY,
                MoveJudgement.MISTAKE,
                MoveJudgement.BLUNDER,
            ).forEach { j ->
                val c = judgementColor(j, dark)
                val label = judgementPlural(j)
                // From the PGN's evaluations alone !! and ! are not known: a dash, not a zero.
                val unknown = state.nodes == 0UL && (j == MoveJudgement.BRILLIANT || j == MoveJudgement.GREAT)
                val cell: @Composable (Boolean, UInt, String) -> Unit = { isWhite, count, side ->
                    val active = count > 0u && !unknown
                    val shown = if (unknown) "\u2014" else count.toString()
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .height(48.dp)
                            .then(
                                if (active) {
                                    Modifier
                                        .clickable(onClickLabel = next) { onError(isWhite, j) }
                                        .semantics {
                                            role = Role.Button
                                            contentDescription = "$label, $side, $count"
                                        }
                                } else {
                                    Modifier.clearAndSetSemantics { contentDescription = "$label, $side, $shown" }
                                },
                            ),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.End,
                    ) {
                        Text(
                            shown,
                            style = MaterialTheme.typography.titleMedium.merge(Tabular),
                            color = if (active) c else variant,
                        )
                        Box(Modifier.width(24.dp), contentAlignment = Alignment.CenterEnd) {
                            if (active) Icon(Icons.Rounded.ChevronRight, null, tint = variant, modifier = Modifier.size(20.dp))
                        }
                    }
                }
                val whiteSide = stringResource(R.string.tag_white)
                val blackSide = stringResource(R.string.tag_black)
                ReportRow(
                    label = {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(judgementSymbol(j), color = c, fontWeight = FontWeight.Bold, modifier = Modifier.width(28.dp))
                            Text(label, style = MaterialTheme.typography.bodyLarge)
                        }
                    },
                    white = { cell(true, state.white.inaccuraciesOf(j), whiteSide) },
                    black = { cell(false, state.black.inaccuraciesOf(j), blackSide) },
                )
            }
            ReportRow(
                label = { Text(stringResource(R.string.review_acpl), style = MaterialTheme.typography.bodyMedium, color = variant) },
                white = { ReportNumber(state.white.acpl?.toString() ?: "\u2014") },
                black = { ReportNumber(state.black.acpl?.toString() ?: "\u2014") },
                height = null,
            )
            listOf(
                Triple(R.string.review_opening, state.white.opening, state.black.opening),
                Triple(R.string.review_middlegame, state.white.middlegame, state.black.middlegame),
                Triple(R.string.review_endgame, state.white.endgame, state.black.endgame),
            ).filter { it.second != null || it.third != null }.forEach { (res, w, b) ->
                ReportRow(
                    label = { Text(stringResource(res), style = MaterialTheme.typography.bodyMedium, color = variant) },
                    white = { ReportNumber(percent(w)) },
                    black = { ReportNumber(percent(b)) },
                    height = 40,
                )
            }
            Spacer(Modifier.height(12.dp))
            Text(
                if (state.nodes == 0UL) {
                    stringResource(R.string.review_source_pgn)
                } else {
                    stringResource(R.string.review_source, state.engine, compactCount(state.nodes))
                },
                style = MaterialTheme.typography.bodySmall,
                color = variant,
            )
            if (state.nodes == 0UL) {
                Text(stringResource(R.string.review_no_notes), style = MaterialTheme.typography.bodySmall, color = variant)
            }
            if (state.stale) {
                Text(stringResource(R.string.review_stale), style = MaterialTheme.typography.bodySmall, color = variant)
            }
            Spacer(Modifier.height(20.dp))
            // With a large font the two buttons go on two lines instead of breaking words.
            FlowRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                TextButton(onClick = onHide) { Text(stringResource(R.string.review_hide)) }
                // Over evaluations of the PGN the engine can make its own; an engine
                // analysis made again would only repeat itself.
                if (state.nodes == 0UL) {
                    OutlinedButton(onClick = onAgain, enabled = !state.running) { Text(stringResource(R.string.review_engine)) }
                }
            }
        }
    }
}

private fun SideReview.inaccuraciesOf(j: MoveJudgement): UInt = when (j) {
    MoveJudgement.INACCURACY -> inaccuracies
    MoveJudgement.MISTAKE -> mistakes
    MoveJudgement.BLUNDER -> blunders
    MoveJudgement.GREAT -> great
    MoveJudgement.BRILLIANT -> brilliant
}

/// A row of the report: its label, then White's and Black's cells, 96 dp each.
@Composable
private fun ReportRow(
    label: @Composable () -> Unit,
    white: @Composable () -> Unit,
    black: @Composable () -> Unit,
    height: Int? = 48,
) {
    Row(
        Modifier.fillMaxWidth().then(if (height != null) Modifier.height(height.dp) else Modifier.padding(vertical = 4.dp)),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.weight(1f)) { label() }
        Box(Modifier.width(96.dp), contentAlignment = Alignment.CenterEnd) { white() }
        Box(Modifier.width(96.dp), contentAlignment = Alignment.CenterEnd) { black() }
    }
}

/// A player over a column: the side's circle and the surname, up to two lines; the full
/// name for TalkBack.
@Composable
private fun PlayerHead(name: String, white: Boolean) {
    Row(
        Modifier.padding(start = 4.dp).semantics(mergeDescendants = true) { contentDescription = name },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        SideDot(white, 12)
        Text(
            name.substringBefore(',').trim(),
            style = MaterialTheme.typography.labelLarge,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(start = 8.dp),
        )
    }
}

@Composable
private fun ReportNumber(text: String, style: TextStyle = MaterialTheme.typography.bodyLarge) {
    Text(text, style = style.merge(Tabular), fontWeight = FontWeight.Medium, modifier = Modifier.padding(end = 24.dp))
}

/// 250 000 → "250k", 1 000 000 → "1M", 1 500 000 → "1.5M" (as on the desktop).
private fun compactCount(n: ULong): String {
    val v = n.toLong()
    return when {
        v >= 1_000_000 && v % 100_000 == 0L -> if (v % 1_000_000 == 0L) "${v / 1_000_000}M" else "%.1fM".format(java.util.Locale.ROOT, v / 1e6)
        v >= 1000 -> "${v / 1000}k"
        else -> v.toString()
    }
}
