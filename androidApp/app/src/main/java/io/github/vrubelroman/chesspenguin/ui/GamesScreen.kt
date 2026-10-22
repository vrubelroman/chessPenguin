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
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.vrubelroman.chesspenguin.R
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
    LaunchedEffect(db) {
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
        val found = withContext(Dispatchers.Default) { d.find(query) }
        rows.clear()
        shown = found
        // New results start at the top.
        list.scrollToItem(0)
        app.shown[db] = found
    }
    val scroll = TopAppBarDefaults.pinnedScrollBehavior()
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
                    }
                },
                scrollBehavior = scroll,
            )
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
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
                else -> LazyColumn(Modifier.fillMaxSize(), state = list) {
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
                        GameRowView(loaded?.getOrNull(place % CHUNK)) { onGame(place, query) }
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
private fun openProblem(e: Throwable): String = when (e) {
    is io.github.vrubelroman.chesspenguin.data.MissingPart -> stringResource(R.string.open_missing_part, e.file)
    is SecurityException, is java.io.FileNotFoundException, is ApiException.Io -> stringResource(R.string.open_no_access)
    is ApiException.Corrupt -> stringResource(R.string.open_corrupt)
    is ApiException.Empty -> stringResource(R.string.open_empty)
    is ApiException.Unsupported -> stringResource(R.string.open_unsupported, e.detail)
    else -> ""
}

/// One game: "White – Black" and the result, then event · year · ECO (§6.2). An
/// unfinished game ("*", most book positions) shows no result.
@Composable
private fun GameRowView(row: GameRow?, onClick: () -> Unit) {
    Column(
        Modifier
            .fillMaxWidth()
            .height(64.dp)
            .clickable(enabled = row != null, onClick = onClick)
            .padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        if (row == null) return@Column
        Row(verticalAlignment = Alignment.CenterVertically) {
            Row(Modifier.weight(1f)) {
                Text(
                    shortName(row.white),
                    style = MaterialTheme.typography.bodyLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                // A study or a book position often has a title for White and no Black.
                if (shortName(row.black) != "?" || shortName(row.white) == "?") {
                    Text(" – ", style = MaterialTheme.typography.bodyLarge)
                    Text(
                        shortName(row.black),
                        style = MaterialTheme.typography.bodyLarge,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                }
            }
            val result = shortResult(row.result)
            if (result != "*") {
                Text(
                    result,
                    style = MaterialTheme.typography.bodyLarge.merge(Tabular),
                    textAlign = TextAlign.End,
                    modifier = Modifier.width(48.dp).padding(start = 8.dp),
                )
            }
        }
        val details = joinKnown(row.event, year(row.date), row.eco)
        if (details.isNotEmpty()) {
            Text(
                details,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}
