package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.foundation.layout.heightIn
import kotlinx.coroutines.flow.first
import androidx.compose.runtime.snapshotFlow
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.Source
import androidx.compose.ui.zIndex
import androidx.compose.ui.platform.LocalResources
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material.icons.rounded.Download
import io.github.vrubelroman.chesspenguin.theme.Tabular
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import uniffi.mobile_api.ApiException
import uniffi.mobile_api.Database
import uniffi.mobile_api.GameRow

private const val CHUNK = 100

/// The games of a database, with a search (android-ui.md §6).
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun GamesScreen(db: String, onBack: () -> Unit, onGame: (place: Int, query: String) -> Unit) {
    val app = app()
    val entries by app.library.entries.collectAsState()
    val entry = entries.firstOrNull { it.id == db }
    var database by remember { mutableStateOf<Database?>(null) }
    var failed by remember { mutableStateOf<Throwable?>(null) }
    // Read again when a download brought new games into it (android-ui.md §23).
    LaunchedEffect(db, entry?.games, entry?.updated) {
        app.library.database(db).onSuccess { database = it }.onFailure { failed = it }
    }
    var searching by rememberSaveable { mutableStateOf(false) }
    var query by rememberSaveable { mutableStateOf("") }
    var shown by remember { mutableStateOf<List<UInt>?>(null) }
    val list = rememberLazyListState()
    val rows = remember { mutableStateMapOf<Int, List<GameRow>>() }
    LaunchedEffect(database, query) {
        val d = database ?: return@LaunchedEffect
        if (query.isNotEmpty()) delay(250)
        val found = withContext(Dispatchers.Default) { app.library.listed(db, d, query) }
        rows.clear()
        shown = found
        // Once the new list is laid out (else the list keeps its place by a row's key).
        snapshotFlow { list.layoutInfo.totalItemsCount }.first { it == found.size }
        // Back from a game: it in sight; new results start at the top.
        val last = app.lastOpened[db]?.takeIf { it.first == query }?.second?.takeIf { it < found.size }
        val first = list.firstVisibleItemIndex
        val seen = list.layoutInfo.visibleItemsInfo.size.takeIf { it > 0 } ?: 6
        when {
            last == null -> list.scrollToItem(0)
            // Already on the screen (the list kept its place): it stays where it is.
            last in first until first + seen - 1 -> {}
            else -> list.scrollToItem((last - 3).coerceAtLeast(0))
        }
        app.shown[db] = found
    }
    val scroll = TopAppBarDefaults.pinnedScrollBehavior()
    val download = app.downloads.views.collectAsState().value[db]
    val snackbar = remember { SnackbarHostState() }
    UpdateMessages(snackbar)
    val resources = LocalResources.current
    // How a download of this database ended (§23.3); the list reads it again by itself.
    LaunchedEffect(db) {
        app.downloads.finished.collect { f ->
            if (f.id != db) return@collect
            val text = finishedText(resources, f) ?: return@collect
            snackbar.currentSnackbarData?.dismiss()
            snackbar.showSnackbar(text)
        }
    }
    val focus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    BackHandler(searching) {
        searching = false
        query = ""
    }

    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection),
        topBar = {
            TopAppBar(
                navigationIcon = {
                    IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Rounded.ArrowBack, stringResource(R.string.back)) }
                },
                title = {
                    if (searching) {
                        TextField(
                            value = query,
                            onValueChange = { query = it },
                            placeholder = { Text(stringResource(R.string.search_hint)) },
                            singleLine = true,
                            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                            keyboardActions = KeyboardActions(onSearch = { keyboard?.hide() }),
                            colors = TextFieldDefaults.colors(
                                focusedContainerColor = Color.Transparent,
                                unfocusedContainerColor = Color.Transparent,
                                focusedIndicatorColor = Color.Transparent,
                                unfocusedIndicatorColor = Color.Transparent,
                            ),
                            modifier = Modifier.fillMaxWidth().focusRequester(focus),
                        )
                        LaunchedEffect(Unit) { focus.requestFocus() }
                    } else {
                        Column {
                            Text(entry?.let { displayName(it) } ?: "", maxLines = 1, overflow = TextOverflow.Ellipsis)
                            database?.let { d ->
                                val n = d.len().toInt()
                                val found = shown?.size
                                Text(
                                    if (query.isNotEmpty() && found != null) {
                                        stringResource(R.string.found_of, groupDigits(found), groupDigits(n))
                                    } else if (download?.state?.running == true) {
                                        // The games come as the download goes, as on the list of
                                        // databases; the list itself is read again when it ends.
                                        stringResource(R.string.online_downloading, groupDigits((entry?.games ?: n) + download.state.added.toInt()))
                                    } else {
                                        pluralStringResource(R.plurals.games_count, n, groupDigits(n))
                                    },
                                    style = MaterialTheme.typography.bodyMedium.merge(Tabular),
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                        }
                    }
                },
                actions = {
                    if (searching) {
                        IconButton(onClick = {
                            if (query.isEmpty()) searching = false else query = ""
                        }) { Icon(Icons.Rounded.Close, stringResource(R.string.clear)) }
                    } else if (database != null) {
                        IconButton(onClick = { searching = true }) { Icon(Icons.Rounded.Search, stringResource(R.string.search)) }
                        // A downloaded database: its new games from here, where they are
                        // looked for (android-ui.md §23.3); ✕ while they come.
                        if (entry?.source == Source.Online) {
                            if (download?.state?.running == true) {
                                IconButton(onClick = { app.downloads.cancel(db) }) { Icon(Icons.Rounded.Close, stringResource(R.string.online_stop_download)) }
                            } else {
                                IconButton(onClick = { entry.let { app.downloads.update(it) } }) { Icon(Icons.Rounded.Download, stringResource(R.string.online_update)) }
                            }
                        }
                    }
                },
                scrollBehavior = scroll,
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            download?.takeIf { it.state.running }?.let { v ->
                val share = downloadShare(v)
                val bar = Modifier.fillMaxWidth().height(2.dp).align(Alignment.TopStart).zIndex(1f)
                if (share != null) LinearProgressIndicator(progress = { share }, modifier = bar) else LinearProgressIndicator(bar)
            }
            val games = shown
            when {
                failed != null -> Message(stringResource(R.string.open_failed), openProblem(failed!!)) {
                    // The way out: take the database off the list, add the file again.
                    TextButton(onClick = {
                        app.library.remove(db)?.let { app.library.forget(it.second) }
                        onBack()
                    }) { Text(stringResource(R.string.remove)) }
                }
                games == null -> Column(Modifier.align(Alignment.Center), horizontalAlignment = Alignment.CenterHorizontally) {
                    CircularProgressIndicator()
                    Spacer(Modifier.height(16.dp))
                    Text(stringResource(R.string.reading), color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                games.isEmpty() -> Message(stringResource(R.string.nothing_found), "")
                else -> {
                    // Wide enough for the database's largest number.
                    val numberWidth = numberWidth(database?.len()?.toInt() ?: games.size)
                    val last = app.lastOpened[db]?.takeIf { it.first == query }?.second
                    LazyColumn(Modifier.fillMaxSize(), state = list) {
                    items(games.size, key = { games[it].toLong() }) { place ->
                        val chunk = place / CHUNK
                        val loaded = rows[chunk]
                        LaunchedEffect(chunk, games) {
                            if (rows[chunk] == null) {
                                val d = database ?: return@LaunchedEffect
                                val part = games.subList(chunk * CHUNK, minOf(games.size, (chunk + 1) * CHUNK))
                                val read = withContext(Dispatchers.Default) { d.rowsOf(part) }
                                // Rows of a list replaced meanwhile (a new search) are not this one's.
                                if (shown === games) rows[chunk] = read
                            }
                        }
                        GameRowView(
                            games[place].toInt() + 1,
                            numberWidth,
                            loaded?.getOrNull(place % CHUNK),
                            lastOpened = place == last,
                            divider = place < games.size - 1,
                            query = query,
                        ) { onGame(place, query) }
                    }
                }
                }
            }
        }
    }
}

@Composable
private fun Message(title: String, detail: String, action: (@Composable () -> Unit)? = null) {
    Column(Modifier.fillMaxSize().padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
        Text(title, style = MaterialTheme.typography.titleLarge, textAlign = TextAlign.Center)
        if (detail.isNotEmpty()) {
            Spacer(Modifier.height(8.dp))
            Text(detail, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
        }
        if (action != null) {
            Spacer(Modifier.height(16.dp))
            action()
        }
    }
}

/// Why a database did not open, in plain words with the next step (§11).
@Composable
fun openProblem(e: Throwable): String = when (e) {
    is io.github.vrubelroman.chesspenguin.data.NoGames -> stringResource(R.string.file_no_games)
    is io.github.vrubelroman.chesspenguin.data.MissingPart -> stringResource(R.string.open_missing_part, e.file)
    is SecurityException, is java.io.FileNotFoundException, is ApiException.Io -> stringResource(R.string.open_no_access)
    is ApiException.Corrupt -> stringResource(R.string.open_corrupt)
    is ApiException.Empty -> stringResource(R.string.open_empty)
    is ApiException.Unsupported -> stringResource(R.string.open_unsupported, e.detail)
    else -> ""
}

/// The width of the numbers column: the digits of `count`, grouped as shown.
@Composable
private fun numberWidth(count: Int): androidx.compose.ui.unit.Dp {
    val style = MaterialTheme.typography.bodySmall.merge(Tabular)
    val measurer = androidx.compose.ui.text.rememberTextMeasurer()
    val density = androidx.compose.ui.platform.LocalDensity.current
    return remember(count, style, density) {
        with(density) { measurer.measure(groupDigits(count.coerceAtLeast(1)), style).size.width.toDp() }
    }
}

/// One game (§6.2, android-lists.md §3): its number, White and Black each on a line of
/// their own (a disc of the side, the rating, the share of the result), then event · year
/// and the ECO on a tag. A thin line under it; the game last opened is marked. The row
/// is read as one sentence by TalkBack.
@Composable
private fun GameRowView(
    number: Int,
    numberWidth: androidx.compose.ui.unit.Dp,
    row: GameRow?,
    lastOpened: Boolean,
    divider: Boolean,
    query: String,
    onClick: () -> Unit,
) {
    // 6 + 24 + 24 + 20 + 6 = 80 dp at the normal font size; the lines grow with a larger font.
    val nameLine = lineHeight(MaterialTheme.typography.bodyLarge, 24.dp)
    val detailsLine = lineHeight(MaterialTheme.typography.bodyMedium, 20.dp)
    val lastText = stringResource(R.string.last_opened)
    val said = row?.let { rowDescription(number, it) + if (lastOpened) ". $lastText" else "" } ?: ""
    val colors = MaterialTheme.colorScheme
    Box(
        Modifier
            .fillMaxWidth()
            .height(12.dp + nameLine * 2 + detailsLine)
            .background(if (lastOpened) colors.surfaceContainer else Color.Transparent)
            .clickable(enabled = row != null, onClick = onClick)
            .clearAndSetSemantics { if (row != null) contentDescription = said },
    ) {
        Row(Modifier.fillMaxSize().padding(horizontal = 16.dp, vertical = 6.dp), verticalAlignment = Alignment.Top) {
            // Level with White's line.
            Box(Modifier.width(numberWidth).height(nameLine), contentAlignment = Alignment.CenterEnd) {
                Text(
                    groupDigits(number),
                    style = MaterialTheme.typography.bodySmall.merge(Tabular),
                    color = colors.onSurfaceVariant,
                    maxLines = 1,
                )
            }
            Spacer(Modifier.width(12.dp))
            GameRowText(row, nameLine, detailsLine, query, Modifier.weight(1f))
        }
        if (lastOpened) {
            Box(
                Modifier.align(Alignment.CenterStart).width(3.dp).fillMaxHeight()
                    .background(colors.primary, RoundedCornerShape(topEnd = 2.dp, bottomEnd = 2.dp)),
            )
        }
        // From the discs to 16 dp before the edge; the number stays in the margin.
        if (divider) {
            Box(
                Modifier.align(Alignment.BottomStart).padding(start = 16.dp + numberWidth + 12.dp, end = 16.dp)
                    .fillMaxWidth().height(1.dp).background(colors.outlineVariant),
            )
        }
    }
}

@Composable
private fun GameRowText(row: GameRow?, nameLine: androidx.compose.ui.unit.Dp, detailsLine: androidx.compose.ui.unit.Dp, query: String, modifier: Modifier) {
    Column(modifier) {
        if (row == null) return@Column
        val style = MaterialTheme.typography.bodyLarge
        val muted = MaterialTheme.colorScheme.onSurfaceVariant
        val white = row.white.trim().takeIf { it.isNotEmpty() && it != "?" }
        val black = row.black.trim().takeIf { it.isNotEmpty() && it != "?" }
        val study = white != null && black == null
        if (study) {
            // A study or a book position: a title for White and no Black — two lines of it.
            // From the top, level with the number, not centred in the two lines; no discs:
            // these are not players.
            Box(Modifier.height(nameLine * 2)) {
                PlayerName(white, style, maxLines = 2, abbreviate = false, highlight = query)
            }
        } else {
            ListPlayer(white, elo(row.whiteElo), sideResult(row.result, true), true, style, nameLine, query)
            ListPlayer(black, elo(row.blackElo), sideResult(row.result, false), false, style, nameLine, query)
        }
        val details = joinKnown(row.event, year(row.date))
        val eco = row.eco.trim().takeIf { it.isNotEmpty() && it != "?" }
        if (details.isNotEmpty() || eco != null) {
            // Under the names (past the discs); the ECO tag never gives way, the event does.
            Row(Modifier.height(detailsLine).padding(start = if (study) 0.dp else 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(
                    highlighted(details, query),
                    style = MaterialTheme.typography.bodyMedium,
                    color = muted,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                if (eco != null) {
                    Spacer(Modifier.width(8.dp))
                    EcoTag(eco, marked = query.split(' ').any { it.isNotBlank() && eco.startsWith(it.trim(), ignoreCase = true) })
                }
            }
        }
    }
}

/// One player in the list: the disc of the side, the name (the winner's a little
/// heavier), the rating, the share of the result on its tile.
@Composable
private fun ListPlayer(
    name: String?,
    rating: String,
    result: String?,
    white: Boolean,
    style: androidx.compose.ui.text.TextStyle,
    height: androidx.compose.ui.unit.Dp,
    query: String,
) {
    val won = result == "1"
    Row(Modifier.height(height), verticalAlignment = Alignment.CenterVertically) {
        SideDisc(white, 8.dp)
        Spacer(Modifier.width(8.dp))
        Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
            PlayerName(
                name ?: "?",
                if (won) style.copy(fontWeight = androidx.compose.ui.text.font.FontWeight.Medium) else style,
                Modifier.weight(1f, fill = false),
                if (name == null) MaterialTheme.colorScheme.onSurfaceVariant else Color.Unspecified,
                highlight = query,
            )
            if (rating.isNotEmpty()) {
                Spacer(Modifier.width(8.dp))
                Text(rating, style = MaterialTheme.typography.bodyMedium.merge(Tabular), color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
            }
        }
        Spacer(Modifier.width(8.dp))
        ResultTile(result)
    }
}

/// The opening's code on a small outlined tag at the right of the details line; filled
/// like the other matches when it is what was searched for.
@Composable
private fun EcoTag(eco: String, marked: Boolean) {
    val shape = RoundedCornerShape(4.dp)
    val colors = MaterialTheme.colorScheme
    Box(
        Modifier.heightIn(min = 16.dp).widthIn(min = 32.dp)
            .background(if (marked) colors.primaryContainer else Color.Transparent, shape)
            .border(1.dp, if (marked) colors.primaryContainer else colors.outline, shape)
            .padding(horizontal = 4.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            eco,
            style = MaterialTheme.typography.labelSmall.merge(Tabular),
            color = if (marked) colors.onPrimaryContainer else colors.onSurfaceVariant,
            maxLines = 1,
        )
    }
}

/// "No. 12. White: Carlsen, Magnus, 2830. Black: … . 1-0, White won. Event · 2025 · C42."
@Composable
private fun rowDescription(number: Int, row: GameRow): String {
    val player = { label: String, name: String, rating: String ->
        "$label: " + joinKnownComma(name.trim().takeIf { it.isNotEmpty() } ?: "?", elo(rating).let { if (it.isEmpty()) "" else "Elo $it" })
    }
    val result = when (row.result) {
        "1-0" -> "1-0, " + stringResource(R.string.white_won).lowercase()
        "0-1" -> "0-1, " + stringResource(R.string.black_won).lowercase()
        "1/2-1/2" -> stringResource(R.string.draw)
        else -> ""
    }
    // A study or a book chapter: a title for White and no Black is read as the title.
    val title = row.white.trim().takeIf { it.isNotEmpty() && it != "?" && row.black.trim().let { b -> b.isEmpty() || b == "?" } }
    return listOf(
        stringResource(R.string.game_number, groupDigits(number)),
        title ?: player(stringResource(R.string.tag_white), row.white, row.whiteElo),
        if (title != null) "" else player(stringResource(R.string.tag_black), row.black, row.blackElo),
        result,
        listOf(row.event, year(row.date), row.eco).map { it.trim() }.filter { it.isNotEmpty() && it != "?" }.joinToString(", "),
    ).filter { it.isNotEmpty() }.joinToString(". ")
}

private fun joinKnownComma(vararg parts: String): String = parts.filter { it.isNotEmpty() }.joinToString(", ")

/// A text style's line height in dp (it grows with the font scale), at least `min`.
@Composable
private fun lineHeight(style: androidx.compose.ui.text.TextStyle, min: androidx.compose.ui.unit.Dp): androidx.compose.ui.unit.Dp {
    val h = style.lineHeight
    if (!h.isSp) return min
    return with(androidx.compose.ui.platform.LocalDensity.current) { h.toDp() }.coerceAtLeast(min)
}
