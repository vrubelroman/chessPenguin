package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SecondaryTabRow
import androidx.compose.material3.Tab
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.BoardTheme
import io.github.vrubelroman.chesspenguin.data.PieceSet
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/// A position to see a board and its pieces by: the Ruy Lopez after 3.Bb5.
private const val SAMPLE = "r1bqkbnr/pppp1ppp/2n5/1B2p3/4P3/5N2/PPPP1PPP/RNBQK2R b KQkq - 3 3"

/// "Board and pieces": the boards and the piece sets of the desktop app, chosen at once
/// and seen on a board above them (android-ui.md §9).
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BoardStyleScreen(onBack: () -> Unit) {
    val app = app()
    val context = LocalContext.current
    val prefs by app.settings.prefs.collectAsState()
    val pieces = remember { Pieces(context) }
    val landscape = LocalConfiguration.current.orientation == android.content.res.Configuration.ORIENTATION_LANDSCAPE

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.board_and_pieces)) },
                navigationIcon = {
                    IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Rounded.ArrowBack, stringResource(R.string.back)) }
                },
            )
        },
    ) { padding ->
        val preview: @Composable (Modifier) -> Unit = { modifier ->
            Board(
                fen = SAMPLE,
                flipped = false,
                lastFrom = "f1",
                lastTo = "b5",
                pieces = pieces,
                theme = prefs.board,
                set = prefs.pieces,
                description = boardLabel(prefs.board) + " · " + prefs.pieces.dir,
                animated = false,
                modifier = modifier,
            )
        }
        val choices: @Composable (Modifier) -> Unit = { modifier ->
            Choices(
                board = prefs.board,
                set = prefs.pieces,
                pieces = pieces,
                onBoard = { b -> app.settings.update { it.copy(board = b) } },
                onSet = { p -> app.settings.update { it.copy(pieces = p) } },
                modifier = modifier,
            )
        }
        // The board stays in sight while the choices scroll under it (or beside it).
        if (landscape) {
            Row(Modifier.fillMaxSize().padding(padding)) {
                Box(Modifier.fillMaxSize().weight(1f), contentAlignment = Alignment.Center) {
                    preview(Modifier.padding(16.dp).fillMaxSize())
                }
                choices(Modifier.weight(1.4f))
            }
        } else {
            Column(Modifier.fillMaxSize().padding(padding)) {
                Box(Modifier.fillMaxWidth().padding(vertical = 12.dp), contentAlignment = Alignment.Center) {
                    preview(Modifier.fillMaxWidth(0.6f).widthIn(max = 240.dp))
                }
                choices(Modifier.weight(1f))
            }
        }
    }
}

/// "Wood 2": the board's name in the interface language.
@Composable
fun boardLabel(b: BoardTheme): String = stringResource(b.label) + (b.number?.let { " $it" } ?: "")

/// Two tabs, so that the piece sets are not hidden under 37 boards.
@Composable
private fun Choices(
    board: BoardTheme,
    set: PieceSet,
    pieces: Pieces,
    onBoard: (BoardTheme) -> Unit,
    onSet: (PieceSet) -> Unit,
    modifier: Modifier,
) {
    var tab by rememberSaveable { mutableIntStateOf(0) }
    Column(modifier) {
        SecondaryTabRow(selectedTabIndex = tab) {
            Tab(selected = tab == 0, onClick = { tab = 0 }, text = { Text(stringResource(R.string.board)) })
            Tab(selected = tab == 1, onClick = { tab = 1 }, text = { Text(stringResource(R.string.pieces_tab)) })
        }
        LazyVerticalGrid(
            GridCells.Adaptive(88.dp),
            Modifier.fillMaxSize(),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(12.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            if (tab == 0) {
                items(BoardTheme.entries, key = { "b" + it.name }) { b ->
                    Swatch(b, b == board) { onBoard(b) }
                }
            } else {
                items(PieceSet.entries, key = { "p" + it.name }) { p ->
                    PieceChoice(p, p == set, board, pieces) { onSet(p) }
                }
            }
        }
    }
}

/// The ring around the chosen board or set.
@Composable
private fun Modifier.chosen(selected: Boolean, shape: RoundedCornerShape): Modifier =
    if (selected) border(3.dp, MaterialTheme.colorScheme.primary, shape) else this

/// A board as 2×2 squares (the corner of its image), its name under it.
@Composable
private fun Swatch(b: BoardTheme, selected: Boolean, onClick: () -> Unit) {
    val context = LocalContext.current
    var side by remember { mutableIntStateOf(0) }
    val corner by produceState<ImageBitmap?>(null, b, side) {
        val file = b.texture ?: return@produceState
        if (side > 0) value = withContext(Dispatchers.IO) { boardCorner(context, file, side) }
    }
    val shape = RoundedCornerShape(6.dp)
    Column(
        Modifier
            .clip(RoundedCornerShape(8.dp))
            .selectable(selected = selected, role = Role.RadioButton, onClick = onClick)
            .padding(4.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Canvas(
            Modifier
                .fillMaxWidth()
                .aspectRatio(1f)
                .chosen(selected, shape)
                .padding(if (selected) 5.dp else 0.dp)
                .clip(RoundedCornerShape(if (selected) 2.dp else 6.dp))
                .onSizeChanged { side = it.width },
        ) {
            val image = corner
            if (image != null) {
                drawImage(image, dstOffset = IntOffset.Zero, dstSize = IntSize(size.width.toInt(), size.height.toInt()))
            } else {
                val sq = size.width / 2
                // a1 dark at the bottom left, as on the board.
                drawRect(b.light, Offset(0f, 0f), Size(sq, sq))
                drawRect(b.dark, Offset(sq, 0f), Size(sq, sq))
                drawRect(b.dark, Offset(0f, sq), Size(sq, sq))
                drawRect(b.light, Offset(sq, sq), Size(sq, sq))
            }
        }
        Label(boardLabel(b), selected)
    }
}

/// A set as its white knight on a light square and black queen on a dark one of the
/// chosen board (a dark set stays readable), its name under them.
@Composable
private fun PieceChoice(p: PieceSet, selected: Boolean, board: BoardTheme, pieces: Pieces, onClick: () -> Unit) {
    val shape = RoundedCornerShape(6.dp)
    Column(
        Modifier
            .clip(RoundedCornerShape(8.dp))
            .selectable(selected = selected, role = Role.RadioButton, onClick = onClick)
            .padding(4.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Canvas(
            Modifier
                .fillMaxWidth()
                .aspectRatio(2f)
                .chosen(selected, shape)
                .padding(if (selected) 5.dp else 0.dp)
                .clip(RoundedCornerShape(if (selected) 2.dp else 6.dp)),
        ) {
            val sq = size.height
            val px = sq.toInt()
            val gap = (size.width - 2 * sq) / 2
            drawRect(board.light, Offset(gap, 0f), Size(sq, sq))
            drawRect(board.dark, Offset(gap + sq, 0f), Size(sq, sq))
            pieces.bitmap(p, 'N', px)?.let { drawImage(it, dstOffset = IntOffset(gap.toInt(), 0), dstSize = IntSize(px, px)) }
            pieces.bitmap(p, 'q', px)?.let { drawImage(it, dstOffset = IntOffset((gap + sq).toInt(), 0), dstSize = IntSize(px, px)) }
        }
        Label(p.dir, selected)
    }
}

@Composable
private fun Label(text: String, selected: Boolean) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        color = if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
        textAlign = TextAlign.Center,
        maxLines = 2,
        overflow = TextOverflow.Ellipsis,
        modifier = Modifier.padding(top = 4.dp),
    )
}
