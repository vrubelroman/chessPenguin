package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.theme.ChessColors
import io.github.vrubelroman.chesspenguin.theme.LocalChessColors
import io.github.vrubelroman.chesspenguin.theme.nagColor
import uniffi.mobile_api.GameData
import uniffi.mobile_api.MoveNode

/// A piece of the notation: the main line as a table of moves, comments and variations
/// as full-width blocks between its rows (as on the desktop, android-ui.md §7.3).
sealed interface Item {
    data class Comment(val text: String) : Item
    data class Moves(val number: Int, val white: Int?, val black: Int?) : Item
    data class Variation(val start: Int, val depth: Int) : Item
    data class Result(val text: String) : Item
}

/// The items of a game, and for each node the item that shows it (to keep it in view).
fun layout(game: GameData, resultText: String): Pair<List<Item>, Map<Int, Int>> {
    val nodes = game.nodes
    val items = ArrayList<Item>()
    val where = HashMap<Int, Int>()
    fun node(i: Int) = nodes[i]
    val root = node(0)
    if (root.commentAfter.isNotEmpty()) items += Item.Comment(root.commentAfter)
    var row: Item.Moves? = null
    fun flush() {
        row?.let { r ->
            items += r
            r.white?.let { where[it] = items.size - 1 }
            r.black?.let { where[it] = items.size - 1 }
        }
        row = null
    }
    fun markVariation(start: Int, at: Int) {
        // Every node of the variation (its whole subtree) is shown by this item.
        val stack = ArrayDeque(listOf(start))
        while (stack.isNotEmpty()) {
            val n = stack.removeLast()
            where[n] = at
            node(n).children.forEach { stack.add(it.toInt()) }
        }
    }
    var parent = 0
    while (true) {
        val kids = node(parent).children
        if (kids.isEmpty()) break
        val id = kids[0].toInt()
        val n = node(id)
        if (n.commentBefore.isNotEmpty()) {
            flush()
            items += Item.Comment(n.commentBefore)
        }
        if (n.white) {
            flush()
            row = Item.Moves(n.moveNumber.toInt(), id, null)
        } else {
            row = (row ?: Item.Moves(n.moveNumber.toInt(), null, null)).copy(black = id)
        }
        val variations = kids.drop(1).map { it.toInt() }
        val interrupted = n.commentAfter.isNotEmpty() || variations.isNotEmpty()
        if (interrupted || !n.white) {
            flush()
            if (n.commentAfter.isNotEmpty()) items += Item.Comment(n.commentAfter)
            for (v in variations) {
                items += Item.Variation(v, 1)
                markVariation(v, items.size - 1)
            }
        }
        parent = id
    }
    flush()
    if (resultText.isNotEmpty()) items += Item.Result(resultText)
    return items to where
}

private fun label(n: MoveNode, number: Boolean, san: (String) -> AnnotatedString): AnnotatedString = when {
    n.white -> AnnotatedString("${n.moveNumber}.") + san(n.san)
    number -> AnnotatedString("${n.moveNumber}…") + san(n.san)
    else -> san(n.san)
}

/// A variation as flowing text: its moves (each a link), comments in italics, nested
/// variations in parentheses.
@Composable
private fun variationText(
    game: GameData,
    start: Int,
    current: Int,
    chess: ChessColors,
    san: (String) -> AnnotatedString,
    onMove: (Int) -> Unit,
): AnnotatedString {
    val nodes = game.nodes
    val comment = chess.comment
    val onSurface = MaterialTheme.colorScheme.onSurface
    return remember(game, start, current, san) {
        buildAnnotatedString {
            // A space goes before each word, except right after "(".
            var space = false
            fun word(write: () -> Unit) {
                if (space) append(' ')
                write()
                space = true
            }
            fun line(first: Int) {
                var id = first
                var number = true
                while (true) {
                    val n = nodes[id]
                    if (n.commentBefore.isNotEmpty()) {
                        word { withStyle(SpanStyle(color = comment, fontStyle = FontStyle.Italic)) { append(n.commentBefore) } }
                        number = true
                    }
                    val style = if (id == current) {
                        SpanStyle(background = chess.currentMove, color = onSurface, fontWeight = FontWeight.Medium)
                    } else {
                        SpanStyle(color = onSurface)
                    }
                    val target = id
                    word {
                        withLink(LinkAnnotation.Clickable("m$id", TextLinkStyles(style)) { onMove(target) }) {
                            append(label(n, number, san))
                        }
                        if (n.nagSymbols.isNotEmpty()) {
                            // On the highlighted move the sign takes the text color.
                            val c = if (id == current) onSurface else n.nags.firstOrNull()?.let { nagColor(it.toInt(), chess.dark) } ?: onSurface
                            withStyle(SpanStyle(color = c)) { append(n.nagSymbols) }
                        }
                    }
                    number = false
                    if (n.commentAfter.isNotEmpty()) {
                        word { withStyle(SpanStyle(color = comment, fontStyle = FontStyle.Italic)) { append(n.commentAfter) } }
                        number = true
                    }
                    // The alternatives to this move (but not to the line's first move: those
                    // belong to the level above).
                    if (id != first) {
                        val parent = n.parent?.toInt()
                        if (parent != null) {
                            for (v in nodes[parent].children.drop(1)) {
                                word { append('(') }
                                space = false
                                line(v.toInt())
                                append(')')
                                number = true
                            }
                        }
                    }
                    val next = n.children.firstOrNull() ?: break
                    id = next.toInt()
                }
            }
            line(start)
        }
    }
}

/// The notation under the board (android-ui.md §7.3): the current move highlighted
/// and kept in view; a tap on a move goes there.
@Composable
fun Notation(
    game: GameData,
    items: List<Item>,
    where: Map<Int, Int>,
    current: Int,
    state: LazyListState,
    san: (String) -> AnnotatedString,
    onMove: (Int) -> Unit,
    modifier: Modifier = Modifier,
) {
    val chess = LocalChessColors.current
    LaunchedEffect(current) {
        val at = where[current] ?: if (current == 0) 0 else return@LaunchedEffect
        val visible = state.layoutInfo.visibleItemsInfo
        if (visible.none { it.index == at } || visible.firstOrNull()?.index == at || visible.lastOrNull()?.index == at) {
            // Keep the current move about the middle of what is visible.
            state.animateScrollToItem((at - 2).coerceAtLeast(0))
        }
    }
    LazyColumn(modifier, state = state, contentPadding = PaddingValues(bottom = 8.dp)) {
        itemsIndexed(items) { _, item ->
            when (item) {
                is Item.Comment -> Text(
                    item.text,
                    color = chess.comment,
                    fontSize = 15.sp,
                    lineHeight = 22.sp,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 4.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(MaterialTheme.colorScheme.surfaceContainerLow)
                        .padding(horizontal = 12.dp, vertical = 8.dp),
                )
                is Item.Moves -> MovesRow(game, item, current, chess, san, onMove)
                is Item.Variation -> Box(
                    Modifier
                        .fillMaxWidth()
                        .padding(start = (16 + 16 * (item.depth - 1)).dp, end = 16.dp, top = 2.dp, bottom = 2.dp),
                ) {
                    Row(Modifier.height(androidx.compose.foundation.layout.IntrinsicSize.Min)) {
                        Box(Modifier.width(2.dp).fillMaxHeight().background(MaterialTheme.colorScheme.outlineVariant))
                        Text(
                            variationText(game, item.start, current, chess, san, onMove),
                            fontSize = 15.sp,
                            lineHeight = 28.sp,
                            modifier = Modifier.padding(start = 12.dp, end = 4.dp),
                        )
                    }
                }
                is Item.Result -> Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
                    Text(
                        item.text,
                        style = MaterialTheme.typography.labelLarge,
                        modifier = Modifier
                            .clip(RoundedCornerShape(8.dp))
                            .background(MaterialTheme.colorScheme.surfaceContainerHigh)
                            .padding(horizontal = 16.dp, vertical = 8.dp),
                    )
                }
            }
        }
    }
}

@Composable
private fun MovesRow(
    game: GameData,
    row: Item.Moves,
    current: Int,
    chess: ChessColors,
    san: (String) -> AnnotatedString,
    onMove: (Int) -> Unit,
) {
    Row(Modifier.fillMaxWidth().height(48.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(
            "${row.number}",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontSize = 15.sp,
            modifier = Modifier.width(40.dp).padding(end = 8.dp),
            textAlign = androidx.compose.ui.text.style.TextAlign.End,
        )
        MoveCell(game, row.white, true, current, chess, san, onMove, Modifier.weight(1f))
        MoveCell(game, row.black, false, current, chess, san, onMove, Modifier.weight(1f))
    }
}

@Composable
private fun MoveCell(
    game: GameData,
    id: Int?,
    white: Boolean,
    current: Int,
    chess: ChessColors,
    san: (String) -> AnnotatedString,
    onMove: (Int) -> Unit,
    modifier: Modifier,
) {
    val selected = id != null && id == current
    // TalkBack reads "4, White, dxe5", and whether it is the current move.
    val spoken = id?.let {
        val n = game.nodes[it]
        stringResource(if (white) R.string.move_white else R.string.move_black, n.moveNumber.toInt(), n.san + n.nagSymbols)
    }
    val currentLabel = stringResource(R.string.current_move)
    // The whole cell takes the tap; the fill only hugs the move.
    Box(
        modifier
            .fillMaxHeight()
            .then(if (id != null) Modifier.clickable { onMove(id) } else Modifier)
            .then(
                if (spoken != null) {
                    Modifier.clearAndSetSemantics {
                        contentDescription = spoken
                        role = Role.Button
                        this.selected = selected
                        if (selected) stateDescription = currentLabel
                        onClick { onMove(id); true }
                    }
                } else {
                    Modifier
                },
            )
            .padding(start = 4.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        if (id == null) {
            // "4. …  Nf6": only White's empty cell says that a move is there.
            if (white) Text("…", color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 17.sp, modifier = Modifier.padding(horizontal = 8.dp))
        } else {
            val n = game.nodes[id]
            Row(
                Modifier
                    .clip(RoundedCornerShape(6.dp))
                    .background(if (selected) chess.currentMove else Color.Transparent)
                    .padding(horizontal = 8.dp, vertical = 6.dp),
                horizontalArrangement = Arrangement.spacedBy(1.dp),
            ) {
                Text(san(n.san), fontSize = 17.sp, fontWeight = FontWeight.Medium, color = MaterialTheme.colorScheme.onSurface)
                if (n.nagSymbols.isNotEmpty()) {
                    Text(
                        n.nagSymbols,
                        fontSize = 17.sp,
                        fontWeight = FontWeight.Medium,
                        color = if (selected) {
                            MaterialTheme.colorScheme.onSurface
                        } else {
                            n.nags.firstOrNull()?.let { nagColor(it.toInt(), chess.dark) } ?: MaterialTheme.colorScheme.onSurfaceVariant
                        },
                    )
                }
            }
        }
    }
}
