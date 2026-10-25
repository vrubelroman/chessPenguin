package io.github.vrubelroman.chesspenguin.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.BitmapRegionDecoder
import android.graphics.Canvas
import android.graphics.Rect
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalViewConfiguration
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.caverock.androidsvg.SVG
import io.github.vrubelroman.chesspenguin.data.BoardTheme
import io.github.vrubelroman.chesspenguin.data.PieceSet
import io.github.vrubelroman.chesspenguin.theme.LocalChessColors
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/// The 64 squares of a FEN's placement, a8 first: the piece letter or ' '.
fun placement(fen: String): CharArray {
    val out = CharArray(64) { ' ' }
    var i = 0
    for (c in fen.substringBefore(' ')) {
        when {
            c == '/' -> {}
            c.isDigit() -> i += c - '0'
            i < 64 -> out[i++] = c
        }
    }
    return out
}

/// "e4" → its index in [placement] (a8 = 0).
fun squareIndex(square: String?): Int? {
    if (square == null || square.length < 2) return null
    val file = square[0] - 'a'
    val rank = square[1] - '1'
    if (file !in 0..7 || rank !in 0..7) return null
    return (7 - rank) * 8 + file
}

/// The pieces of the desktop app's sets, drawn from their SVG at a size once.
class Pieces(private val context: Context) {
    private val svgs = HashMap<Pair<PieceSet, Char>, SVG>()
    private val bitmaps = HashMap<Triple<PieceSet, Char, Int>, ImageBitmap>()

    fun bitmap(set: PieceSet, piece: Char, size: Int): ImageBitmap? {
        if (size <= 0) return null
        return bitmaps.getOrPut(Triple(set, piece, size)) {
            val name = (if (piece.isUpperCase()) "w" else "b") + piece.uppercaseChar()
            val svg = svgs.getOrPut(set to piece) { context.assets.open("pieces/${set.dir}/$name.svg").use { SVG.getFromInputStream(it) } }
            val bmp = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888)
            // Without a viewBox (kiwen-suwi, metaltops) AndroidSVG draws a set at its own
            // size, whatever size is asked for.
            if (svg.documentViewBox == null) svg.setDocumentViewBox(0f, 0f, svg.documentWidth, svg.documentHeight)
            svg.setDocumentWidth(size.toFloat())
            svg.setDocumentHeight(size.toFloat())
            svg.renderToCanvas(Canvas(bmp))
            bmp.asImageBitmap()
        }
    }
}

/// A board's image (1024 px) read at about `size` px: a phone's board needs no more, and
/// the whole image would take 4 MB.
fun boardImage(context: Context, file: String, size: Int): ImageBitmap? = runCatching {
    val bytes = context.assets.open("boards/$file").use { it.readBytes() }
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    var sample = 1
    while (bounds.outWidth / (sample * 2) >= size) sample *= 2
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })?.asImageBitmap()
}.getOrNull()

/// The corner of a board's image with a1: 2×2 squares, for the choice of boards.
fun boardCorner(context: Context, file: String, size: Int): ImageBitmap? = runCatching {
    context.assets.open("boards/$file").use { input ->
        val decoder = BitmapRegionDecoder.newInstance(input) ?: return null
        try {
            val q = decoder.width / 4
            var sample = 1
            while (q / (sample * 2) >= size) sample *= 2
            decoder.decodeRegion(Rect(0, decoder.height - q, q, decoder.height), BitmapFactory.Options().apply { inSampleSize = sample })?.asImageBitmap()
        } finally {
            decoder.recycle()
        }
    }
}.getOrNull()

/// "e4" of the square with index `i` of [placement] (a8 = 0).
fun squareName(i: Int): String = "${'a' + i % 8}${'8' - i / 8}"

/// Moves on the board (android-play.md §29): the legal moves of the position in UCI and
/// what to do with the one made. A pawn on the last rank becomes a queen (owner's
/// decision §29.4); `promotion` may name another piece for a from–to pair (the trainer
/// takes the line's own piece).
class BoardInput(
    val legal: List<String>,
    val onMove: (String) -> Unit,
    val promotion: (String) -> Char? = { null },
) {
    /// The squares a piece on `from` can go to.
    fun targets(from: String): Set<String> = legal.filter { it.startsWith(from) }.map { it.substring(2, 4) }.toSet()

    /// The move from `from` to `to`, or null when there is none.
    fun move(from: String, to: String): String? {
        val pair = from + to
        val all = legal.filter { it.startsWith(pair) }
        if (all.isEmpty()) return null
        if (all.size == 1) return all[0]
        val piece = promotion(pair) ?: 'q'
        return all.firstOrNull { it == pair + piece } ?: all.firstOrNull { it == pair + 'q' } ?: all[0]
    }
}

/// An arrow on the board: a comment's `[%cal]` or the engine's best move.
data class BoardArrow(val from: String, val to: String, val color: Color, val weight: Float = 1f)

/// A marked square of a comment (`[%csl]`), drawn as a ring.
data class BoardMark(val square: String, val color: Color)

/// The comment colors of the desktop app (game_view.rs `shape_color`).
fun markColor(letter: String): Color = when (letter) {
    "R" -> Color(136, 32, 32, 170)
    "G" -> Color(21, 120, 27, 170)
    "B" -> Color(0, 48, 136, 170)
    "Y" -> Color(230, 143, 0, 170)
    else -> Color(120, 120, 120, 170)
}

/// The engine's arrows are blue, as on the desktop.
val EngineArrow = Color(0, 48, 136, 160)

/// The board: full width, the chosen board and pieces, the last move lit, the
/// coordinates inside the edge squares (as on Lichess). With `input` a move is made by
/// tapping a piece and then a square, or by dragging it (android-play.md §29.2–29.3);
/// without it the board takes no touches. A move slides into place (250 ms).
@Composable
fun Board(
    fen: String,
    flipped: Boolean,
    lastFrom: String?,
    lastTo: String?,
    pieces: Pieces,
    theme: BoardTheme,
    set: PieceSet,
    description: String,
    modifier: Modifier = Modifier,
    arrows: List<BoardArrow> = emptyList(),
    marks: List<BoardMark> = emptyList(),
    check: Boolean = false,
    animated: Boolean = true,
    input: BoardInput? = null,
) {
    val colors = LocalChessColors.current
    val context = LocalContext.current
    // The image of a textured board, read off the main thread; the flat colors until then.
    var side by remember { mutableIntStateOf(0) }
    val texture by produceState<ImageBitmap?>(null, theme, side) {
        value = null
        val file = theme.texture ?: return@produceState
        if (side > 0) value = withContext(Dispatchers.IO) { boardImage(context, file, side) }
    }
    val measurer = rememberTextMeasurer()
    val squares = remember(fen) { placement(fen) }
    val from = squareIndex(lastFrom)
    val to = squareIndex(lastTo)
    // The piece of the last move slides from its square when the position steps on.
    val slide = remember { Animatable(1f) }
    var previous by remember { mutableStateOf<String?>(null) }
    var animate by remember { mutableStateOf(false) }
    LaunchedEffect(fen) {
        animate = animated && previous != null && from != null && to != null && placement(previous!!).let { p -> p[from] != ' ' && p[to] != squares[to] }
        previous = fen
        if (animate) {
            slide.snapTo(0f)
            slide.animateTo(1f, tween(250, easing = FastOutSlowInEasing))
        } else {
            slide.snapTo(1f)
        }
    }
    val coordStyle = TextStyle(fontSize = 10.sp, fontWeight = FontWeight.Medium)
    val density = LocalDensity.current
    // A move being made: the piece chosen, and while dragged, where the finger is.
    var selected by remember(fen) { mutableStateOf<Int?>(null) }
    var dragAt by remember(fen) { mutableStateOf<Offset?>(null) }
    val slop = LocalViewConfiguration.current.touchSlop
    val currentInput = androidx.compose.runtime.rememberUpdatedState(input)
    val lift = with(density) { 22.dp.toPx() }
    fun indexAt(p: Offset, sq: Float): Int? {
        val col = (p.x / sq).toInt()
        val row = (p.y / sq).toInt()
        if (p.x < 0 || p.y < 0 || col !in 0..7 || row !in 0..7) return null
        return if (flipped) (7 - row) * 8 + (7 - col) else row * 8 + col
    }
    val touch = if (input == null) Modifier else Modifier.pointerInput(fen, flipped) {
        awaitEachGesture {
            val down = awaitFirstDown()
            val inp = currentInput.value ?: return@awaitEachGesture
            val sq = size.width / 8f
            val start = indexAt(down.position, sq) ?: return@awaitEachGesture
            val startName = squareName(start)
            val movable = inp.targets(startName).isNotEmpty()
            val was = selected
            // A tap on a target of the piece chosen makes the move.
            if (was != null && start != was) {
                inp.move(squareName(was), startName)?.let { m ->
                    selected = null
                    inp.onMove(m)
                    return@awaitEachGesture
                }
            }
            if (movable) selected = start
            var dragging = false
            while (true) {
                val e = awaitPointerEvent()
                val c = e.changes.firstOrNull { it.id == down.id } ?: break
                if (!c.pressed) {
                    if (dragging) {
                        val end = indexAt(c.position, sq)
                        dragAt = null
                        val m = end?.let { inp.move(startName, squareName(it)) }
                        if (m != null) {
                            selected = null
                            inp.onMove(m)
                        }
                        // A drop elsewhere: the piece goes back, still chosen.
                    } else if (!movable || start == was) {
                        // A tap on the same piece, an empty square or the other side's
                        // piece: nothing chosen any more.
                        selected = null
                    }
                    c.consume()
                    break
                }
                if (movable && !dragging && (c.position - down.position).getDistance() > slop) dragging = true
                if (dragging) {
                    dragAt = c.position
                    c.consume()
                }
            }
        }
    }
    Canvas(
        modifier
            .aspectRatio(1f)
            .onSizeChanged { side = it.width }
            .then(touch)
            .semantics { contentDescription = description },
    ) {
        val sq = size.width / 8f
        val px = sq.toInt()
        fun cell(i: Int): Offset {
            val row = i / 8
            val col = i % 8
            val (r, c) = if (flipped) (7 - row) to (7 - col) else row to col
            return Offset(c * sq, r * sq)
        }
        val image = texture
        if (image != null) {
            // The squares of the image are the same seen from either side: it is not turned.
            drawImage(image, dstSize = IntSize(size.width.toInt(), size.height.toInt()))
        }
        for (i in 0 until 64) {
            val light = (i / 8 + i % 8) % 2 == 0
            if (image == null) drawRect(if (light) theme.light else theme.dark, cell(i), Size(sq, sq))
            if (i == from || i == to) drawRect(colors.lastMove, cell(i), Size(sq, sq))
        }
        val chosen = selected
        if (chosen != null) drawRect(colors.lastMove.copy(alpha = 0.5f), cell(chosen), Size(sq, sq))
        for (m in marks) {
            val i = squareIndex(m.square) ?: continue
            val c = cell(i)
            drawCircle(m.color, radius = sq * 0.45f, center = Offset(c.x + sq / 2, c.y + sq / 2), style = Stroke(width = sq * 0.08f))
        }
        if (check) {
            // A red glow under the king of the side to move.
            val king = if (fen.substringAfter(' ').startsWith("b")) 'k' else 'K'
            val i = squares.indexOf(king)
            if (i >= 0) {
                val c = cell(i)
                val center = Offset(c.x + sq / 2, c.y + sq / 2)
                drawCircle(
                    Brush.radialGradient(
                        0f to Color(255, 0, 0, 230),
                        0.25f to Color(231, 0, 0, 230),
                        0.89f to Color(169, 0, 0, 0),
                        center = center,
                        radius = sq * 0.6f,
                    ),
                    radius = sq * 0.6f,
                    center = center,
                )
            }
        }
        for (i in 0 until 64) {
            val p = squares[i]
            if (p == ' ') continue
            val bmp = pieces.bitmap(set, p, px) ?: continue
            var at = cell(i)
            // The piece being dragged: faint on its square, drawn under the finger below.
            if (i == chosen && dragAt != null) {
                drawImage(bmp, dstOffset = IntOffset(at.x.toInt(), at.y.toInt()), dstSize = IntSize(px, px), alpha = 0.3f)
                continue
            }
            if (animate && i == to && from != null && slide.value < 1f) {
                val start = cell(from)
                at = Offset(start.x + (at.x - start.x) * slide.value, start.y + (at.y - start.y) * slide.value)
            }
            drawImage(bmp, dstOffset = IntOffset(at.x.toInt(), at.y.toInt()), dstSize = IntSize(px, px))
        }
        // Where the piece chosen can go: a dot, a ring round a capture (as on Lichess).
        if (chosen != null && input != null) {
            val shade = Color(0, 0, 0, 64)
            for (t in input.targets(squareName(chosen))) {
                val i = squareIndex(t) ?: continue
                val c = cell(i) + Offset(sq / 2, sq / 2)
                if (squares[i] != ' ') {
                    drawCircle(shade, radius = sq / 2 - sq * 0.045f, center = c, style = Stroke(width = sq * 0.09f))
                } else {
                    drawCircle(shade, radius = sq * 0.155f, center = c)
                }
            }
        }
        // Over the pieces, so that b, c, f, g stay readable (as on Lichess).
        // Coordinates: files along the bottom edge, ranks along the left edge.
        for (k in 0 until 8) {
            val fileIndex = if (flipped) 7 - k else k
            val rankIndex = if (flipped) k else 7 - k
            val bottomLight = (7 + k) % 2 == 0
            val leftLight = k % 2 == 0
            drawCoord(measurer, ('a' + fileIndex).toString(), coordStyle.copy(color = if (bottomLight) theme.dark else theme.light),
                Offset((k + 1) * sq - with(density) { 8.sp.toPx() }, 8 * sq - with(density) { 13.sp.toPx() }))
            drawCoord(measurer, ('1' + rankIndex).toString(), coordStyle.copy(color = if (leftLight) theme.dark else theme.light),
                Offset(with(density) { 2.sp.toPx() }, k * sq + with(density) { 1.sp.toPx() }))
        }
        for (a in arrows) {
            val f = squareIndex(a.from) ?: continue
            val t = squareIndex(a.to) ?: continue
            drawArrow(cell(f) + Offset(sq / 2, sq / 2), cell(t) + Offset(sq / 2, sq / 2), sq, a)
        }
        // The dragged piece: bigger and lifted above the finger, a shade on the square
        // under it so that one sees where it goes.
        val held = dragAt
        if (held != null && chosen != null) {
            val under = indexAt(held, sq)
            if (under != null) drawCircle(Color(0, 0, 0, 31), radius = sq, center = cell(under) + Offset(sq / 2, sq / 2))
            val big = (px * 1.5f).toInt()
            pieces.bitmap(set, squares[chosen], big)?.let {
                drawImage(it, dstOffset = IntOffset((held.x - big / 2f).toInt(), (held.y - lift - big / 2f).toInt()), dstSize = IntSize(big, big))
            }
        }
    }
}

/// An arrow as on the desktop (board.rs `draw_arrow`): a shaft and a head that stops
/// short of the square's center.
private fun DrawScope.drawArrow(from: Offset, to: Offset, sq: Float, a: BoardArrow) {
    val d = to - from
    val len = d.getDistance()
    if (len < 1f) return
    val dir = d / len
    val width = sq * 0.16f * a.weight
    val headLen = sq * 0.42f * kotlin.math.sqrt(a.weight)
    val headW = sq * 0.38f * kotlin.math.sqrt(a.weight)
    val tip = to - dir * (sq * 0.12f)
    val base = tip - dir * headLen
    val start = from + dir * (sq * 0.2f)
    val normal = Offset(-dir.y, dir.x)
    drawLine(a.color, start, base, strokeWidth = width)
    val head = Path().apply {
        moveTo(tip.x, tip.y)
        lineTo(base.x + normal.x * headW, base.y + normal.y * headW)
        lineTo(base.x - normal.x * headW, base.y - normal.y * headW)
        close()
    }
    drawPath(head, a.color)
}

private fun DrawScope.drawCoord(
    measurer: androidx.compose.ui.text.TextMeasurer,
    text: String,
    style: TextStyle,
    at: Offset,
) {
    drawText(measurer, text, at, style)
}
