package io.github.vrubelroman.chesspenguin.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
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
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.sp
import com.caverock.androidsvg.SVG
import io.github.vrubelroman.chesspenguin.theme.LocalChessColors

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

/// The Merida pieces of the desktop app, drawn from their SVG at a size once.
class Pieces(private val context: Context) {
    private val svgs = HashMap<Char, SVG>()
    private val bitmaps = HashMap<Pair<Char, Int>, ImageBitmap>()

    fun bitmap(piece: Char, size: Int): ImageBitmap? {
        if (size <= 0) return null
        return bitmaps.getOrPut(piece to size) {
            val name = (if (piece.isUpperCase()) "w" else "b") + piece.uppercaseChar()
            val svg = svgs.getOrPut(piece) { context.assets.open("pieces/merida/$name.svg").use { SVG.getFromInputStream(it) } }
            val bmp = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888)
            svg.setDocumentWidth(size.toFloat())
            svg.setDocumentHeight(size.toFloat())
            svg.renderToCanvas(Canvas(bmp))
            bmp.asImageBitmap()
        }
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

/// The board: full width, the colors of the green theme, the last move lit, the
/// coordinates inside the edge squares (as on Lichess). It takes no touches in the first
/// version (docs/design/android-ui.md §7.2). A move slides into place (250 ms).
@Composable
fun Board(
    fen: String,
    flipped: Boolean,
    lastFrom: String?,
    lastTo: String?,
    pieces: Pieces,
    description: String,
    modifier: Modifier = Modifier,
    arrows: List<BoardArrow> = emptyList(),
    marks: List<BoardMark> = emptyList(),
    check: Boolean = false,
    animated: Boolean = true,
) {
    val colors = LocalChessColors.current
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
    Canvas(
        modifier
            .fillMaxWidth()
            .aspectRatio(1f)
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
        for (i in 0 until 64) {
            val light = (i / 8 + i % 8) % 2 == 0
            drawRect(if (light) colors.lightSquare else colors.darkSquare, cell(i), Size(sq, sq))
            if (i == from || i == to) drawRect(colors.lastMove, cell(i), Size(sq, sq))
        }
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
            val bmp = pieces.bitmap(p, px) ?: continue
            var at = cell(i)
            if (animate && i == to && from != null && slide.value < 1f) {
                val start = cell(from)
                at = Offset(start.x + (at.x - start.x) * slide.value, start.y + (at.y - start.y) * slide.value)
            }
            drawImage(bmp, dstOffset = IntOffset(at.x.toInt(), at.y.toInt()), dstSize = IntSize(px, px))
        }
        // Over the pieces, so that b, c, f, g stay readable (as on Lichess).
        // Coordinates: files along the bottom edge, ranks along the left edge.
        for (k in 0 until 8) {
            val fileIndex = if (flipped) 7 - k else k
            val rankIndex = if (flipped) k else 7 - k
            val bottomLight = (7 + k) % 2 == 0
            val leftLight = k % 2 == 0
            drawCoord(measurer, ('a' + fileIndex).toString(), coordStyle.copy(color = if (bottomLight) colors.darkSquare else colors.lightSquare),
                Offset((k + 1) * sq - with(density) { 8.sp.toPx() }, 8 * sq - with(density) { 13.sp.toPx() }))
            drawCoord(measurer, ('1' + rankIndex).toString(), coordStyle.copy(color = if (leftLight) colors.darkSquare else colors.lightSquare),
                Offset(with(density) { 2.sp.toPx() }, k * sq + with(density) { 1.sp.toPx() }))
        }
        for (a in arrows) {
            val f = squareIndex(a.from) ?: continue
            val t = squareIndex(a.to) ?: continue
            drawArrow(cell(f) + Offset(sq / 2, sq / 2), cell(t) + Offset(sq / 2, sq / 2), sq, a)
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
