package io.github.vrubelroman.chesspenguin.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
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
        // New results start at the top.
        list.scrollToItem(0)
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
                    LazyColumn(Modifier.fillMaxSize(), state = list) {
                    items(games.size, key = { games[it].toLong() }) { place ->
                        val chunk = place / CHUNK
                        val loaded = rows[chunk]
                        LaunchedEffect(chunk, games) {
                            if (rows[chunk] == null) {
                                val d = database ?: return@LaunchedEffect
                                val part = games.subList(chunk * CHUNK, minOf(games.size, (chunk + 1) * CHUNK))
                                rows[chunk] = withContext(Dispatchers.Default) { d.rowsOf(part) }
                            }
                        }
                        GameRowView(games[place].toInt() + 1, numberWidth, loaded?.getOrNull(place % CHUNK)) { onGame(place, query) }
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

/// One game (§6.2): its number, White and Black each on a line of their own (with the
/// rating and the share of the result), then event · year · ECO. The row is read as one
/// sentence by TalkBack.
@Composable
private fun GameRowView(number: Int, numberWidth: androidx.compose.ui.unit.Dp, row: GameRow?, onClick: () -> Unit) {
    // 6 + 24 + 24 + 20 + 6 = 80 dp at the normal font size; the lines grow with a larger font.
    val nameLine = lineHeight(MaterialTheme.typography.bodyLarge, 24.dp)
    val detailsLine = lineHeight(MaterialTheme.typography.bodyMedium, 20.dp)
    val said = row?.let { rowDescription(number, it) } ?: ""
    Row(
        Modifier
            .fillMaxWidth()
            .height(12.dp + nameLine * 2 + detailsLine)
            .clickable(enabled = row != null, onClick = onClick)
            .clearAndSetSemantics { if (row != null) contentDescription = said }
            .padding(horizontal = 16.dp, vertical = 6.dp),
        verticalAlignment = Alignment.Top,
    ) {
        // Level with White's line.
        Box(Modifier.width(numberWidth).height(nameLine), contentAlignment = Alignment.CenterEnd) {
            Text(
                groupDigits(number),
                style = MaterialTheme.typography.bodySmall.merge(Tabular),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
            )
        }
        Spacer(Modifier.width(12.dp))
        GameRowText(row, nameLine, detailsLine, Modifier.weight(1f))
    }
}

@Composable
private fun GameRowText(row: GameRow?, nameLine: androidx.compose.ui.unit.Dp, detailsLine: androidx.compose.ui.unit.Dp, modifier: Modifier) {
    Column(modifier) {
        if (row == null) return@Column
        val style = MaterialTheme.typography.bodyLarge
        val muted = MaterialTheme.colorScheme.onSurfaceVariant
        val white = row.white.trim().takeIf { it.isNotEmpty() && it != "?" }
        val black = row.black.trim().takeIf { it.isNotEmpty() && it != "?" }
        if (white != null && black == null) {
            // A study or a book position: a title for White and no Black — two lines of it.
            // From the top, level with the number, not centred in the two lines.
            Box(Modifier.height(nameLine * 2)) {
                PlayerLine(white, elo(row.whiteElo), null, style, maxLines = 2, abbreviate = false)
            }
        } else {
            PlayerLine(white ?: "?", elo(row.whiteElo), sideResult(row.result, true), style, Modifier.height(nameLine), nameColor = if (white == null) muted else androidx.compose.ui.graphics.Color.Unspecified)
            PlayerLine(black ?: "?", elo(row.blackElo), sideResult(row.result, false), style, Modifier.height(nameLine), nameColor = if (black == null) muted else androidx.compose.ui.graphics.Color.Unspecified)
        }
        val details = joinKnown(row.event, year(row.date), row.eco)
        if (details.isNotEmpty()) {
            Text(
                details,
                style = MaterialTheme.typography.bodyMedium,
                color = muted,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.height(detailsLine),
            )
        }
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
