package io.github.vrubelroman.chesspenguin.ui

import android.content.ClipboardManager
import android.text.format.DateUtils
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.background
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.draw.clip
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.EditNote
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.SmartToy
import androidx.compose.material.icons.rounded.MoreVert
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.vrubelroman.chesspenguin.R
import androidx.compose.material3.LinearProgressIndicator
import io.github.vrubelroman.chesspenguin.data.DownloadView
import io.github.vrubelroman.chesspenguin.data.Finished
import io.github.vrubelroman.chesspenguin.data.DbEntry
import io.github.vrubelroman.chesspenguin.data.FolderScan
import io.github.vrubelroman.chesspenguin.data.Source
import uniffi.mobile_api.Database
import kotlinx.coroutines.launch

/// The first screen: the databases on the list (android-ui.md §3).
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DatabasesScreen(onOpen: (String) -> Unit, onAdded: (DbEntry) -> Unit, onSettings: () -> Unit, onOnline: () -> Unit, onNewGame: () -> Unit, onPlay: () -> Unit) {
    val library = app().library
    val downloads = app().downloads
    val views by downloads.views.collectAsState()
    val entries by library.entries.collectAsState()
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val removedText = stringResource(R.string.removed)
    val removedCopyText = stringResource(R.string.removed_copy)
    val removedOwnText = stringResource(R.string.own_removed)
    val update = shownUpdate()
    val offersOffText = stringResource(R.string.upd_offers_off_snack)
    val offersUndoText = stringResource(R.string.undo)
    val updatesForUndo = app().updates
    // The card's ×: offers are off; «Undo» for a slip of the finger (android-ui.md §25.2).
    val onUpdateDismissed: () -> Unit = {
        scope.launch {
            snackbar.currentSnackbarData?.dismiss()
            if (snackbar.showSnackbar(offersOffText, offersUndoText, duration = androidx.compose.material3.SnackbarDuration.Long) == androidx.compose.material3.SnackbarResult.ActionPerformed) {
                updatesForUndo.setOffer(true)
            }
        }
    }
    UpdateMessages(snackbar)
    val removedOnlineText = stringResource(R.string.online_removed)
    val resourcesForSnack = LocalResources.current
    val openText = stringResource(R.string.online_open_short)
    // How a download ended, said while this screen is shown (android-ui.md §23.3).
    LaunchedEffect(Unit) {
        downloads.finished.collect { f ->
            val text = finishedText(resourcesForSnack, f) ?: return@collect
            snackbar.currentSnackbarData?.dismiss()
            val canOpen = f.games > 0 && library.entry(f.id) != null
            if (snackbar.showSnackbar(text, if (canOpen) openText else null, withDismissAction = false) == SnackbarResult.ActionPerformed) onOpen(f.id)
        }
    }
    val undoText = stringResource(R.string.undo)
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) onOpen(library.addFile(uri).id)
    }
    var choosing by remember { mutableStateOf(false) }
    // The folder sheet: shown while `folderOpen`, `scan` null while looking through it.
    // The picked folder outlives a rotation; it is then looked through again.
    var folderOpen by rememberSaveable { mutableStateOf(false) }
    var folder by rememberSaveable { mutableStateOf<android.net.Uri?>(null) }
    var scan by remember { mutableStateOf<FolderScan?>(null) }
    LaunchedEffect(folderOpen, folder) {
        val tree = folder
        if (folderOpen && tree != null && scan?.tree != tree) scan = library.scanFolder(tree)
    }
    val pickFolder = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { tree ->
        if (tree != null) {
            scan = null
            library.keepFolder(tree)
            folder = tree
            folderOpen = true
        }
    }
    val add = { choosing = true }
    var renaming by remember { mutableStateOf<DbEntry?>(null) }
    // Own databases (android-play.md §30): a new one asked for, one to delete asked about.
    var creating by remember { mutableStateOf(false) }
    var deleting by remember { mutableStateOf<DbEntry?>(null) }
    var exporting by remember { mutableStateOf<DbEntry?>(null) }
    val context = androidx.compose.ui.platform.LocalContext.current
    val exportedText = stringResource(R.string.own_exported)
    val exportFailedText = stringResource(R.string.own_export_failed)
    val export = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/x-chess-pgn")) { uri ->
        val e = exporting
        exporting = null
        if (uri == null || e == null) return@rememberLauncherForActivityResult
        scope.launch {
            val ok = withContext(Dispatchers.IO) {
                runCatching {
                    context.contentResolver.openOutputStream(uri, "wt")!!.use { out -> library.ownFile(e).inputStream().use { it.copyTo(out) } }
                }.isSuccess
            }
            snackbar.currentSnackbarData?.dismiss()
            snackbar.showSnackbar(if (ok) exportedText else exportFailedText)
        }
    }
    val own = entries.filter { it.source == Source.Own }
    val theApp = app()
    val adjourned by theApp.maiaGames.current.collectAsState()
    val playRemovedText = stringResource(R.string.play_removed)
    var playing by remember { mutableStateOf(false) }
    val eventText = stringResource(R.string.play_event)
    if (playing) {
        PlaySheet(null, onPlay = { strength, white, name ->
            playing = false
            val s = uniffi.mobile_api.GameSession.newGame("")
            beginMaiaGame(theApp, s, strength, white, name, 0, eventText, null)
            s.close()
            onPlay()
        }, onDismiss = { playing = false })
    }
    val files = entries.filter { it.source != Source.Own }
    val list = rememberLazyListState()
    // The card comes when GitHub answers, after the list is drawn: a list at its top then
    // keeps its first database in place and the card lands above the screen. It is
    // brought into sight; a list scrolled down by the user stays where it is.
    val cardShown = update != null
    LaunchedEffect(cardShown) {
        if (cardShown && list.firstVisibleItemIndex == 1 && list.firstVisibleItemScrollOffset == 0) list.animateScrollToItem(0)
    }
    val expanded by remember { derivedStateOf { list.firstVisibleItemIndex == 0 } }
    val scroll = TopAppBarDefaults.pinnedScrollBehavior()
    // The samples are small: read them at once, so their rows show the number of games.
    LaunchedEffect(Unit) {
        library.entries.value.filter { it.source == Source.Sample && it.games == null }.forEach { library.database(it.id) }
    }

    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection),
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.app_name)) },
                actions = {
                    IconButton(onClick = onSettings) { Icon(Icons.Rounded.Settings, stringResource(R.string.settings)) }
                },
                scrollBehavior = scroll,
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
        floatingActionButton = {
            if (entries.isNotEmpty()) {
                val addLabel = stringResource(R.string.add_database)
                ExtendedFloatingActionButton(
                    // Folded while the list is scrolled: its label is still read out.
                    modifier = Modifier.semantics { contentDescription = addLabel },
                    onClick = add,
                    expanded = expanded,
                    icon = { Icon(Icons.Rounded.Add, null) },
                    text = { Text(stringResource(R.string.add_database)) },
                )
            }
        },
    ) { padding ->
        if (entries.isEmpty()) {
            Column(Modifier.padding(padding)) {
                update?.let { UpdateCard(it, onDismissed = onUpdateDismissed) }
                Empty(Modifier.weight(1f), onAdd = add, onSamples = {
                    library.addSamples()
                    library.entries.value.firstOrNull()?.let { onOpen(it.id) }
                })
            }
        } else {
            LazyColumn(
                Modifier.fillMaxSize(),
                state = list,
                contentPadding = PaddingValues(top = padding.calculateTopPadding(), bottom = padding.calculateBottomPadding() + 88.dp),
            ) {
                // A newer version of the app (android-ui.md §25.2): the first item of the list.
                update?.let { u -> item(key = "update") { UpdateCard(u, onDismissed = onUpdateDismissed, modifier = Modifier.animateItem()) } }
                // A game against Maia put aside (§31.7): go on with it, or let it go.
                adjourned?.takeIf { !it.over }?.let { m ->
                    item(key = "adjourned") {
                        Row(
                            Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp)
                                .background(MaterialTheme.colorScheme.secondaryContainer, RoundedCornerShape(12.dp))
                                .padding(start = 16.dp, end = 4.dp, top = 4.dp, bottom = 4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(
                                stringResource(R.string.play_adjourned, m.strength.toString(), stringResource(if (m.userWhite) R.string.play_as_white else R.string.play_as_black)),
                                Modifier.weight(1f),
                                style = MaterialTheme.typography.bodyMedium,
                            )
                            TextButton(onClick = onPlay) { Text(stringResource(R.string.play_continue)) }
                            IconButton(onClick = {
                                val pgn = theApp.maiaGames.pgn()
                                theApp.maiaGames.clear()
                                scope.launch {
                                    snackbar.currentSnackbarData?.dismiss()
                                    if (snackbar.showSnackbar(playRemovedText, undoText, duration = SnackbarDuration.Long) == SnackbarResult.ActionPerformed && pgn != null) {
                                        theApp.maiaGames.begin(pgn, m)
                                    }
                                }
                            }) { Icon(Icons.Rounded.Close, stringResource(R.string.close)) }
                        }
                    }
                }
                item(key = "actions") {
                    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        FilledTonalButton(onClick = { playing = true }, modifier = Modifier.height(48.dp)) {
                            Icon(Icons.Rounded.SmartToy, null)
                            Spacer(Modifier.size(8.dp))
                            Text(stringResource(R.string.play_with_maia))
                        }
                        FilledTonalButton(onClick = onNewGame, modifier = Modifier.height(48.dp)) {
                            Icon(Icons.Rounded.Add, null)
                            Spacer(Modifier.size(8.dp))
                            Text(stringResource(R.string.new_game))
                        }
                    }
                }
                item(key = "own-head") {
                    Row(Modifier.fillMaxWidth().height(48.dp).padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(stringResource(R.string.own_databases), Modifier.weight(1f), style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary)
                        TextButton(onClick = { creating = true }) { Text(stringResource(R.string.own_new)) }
                    }
                }
                if (own.isEmpty()) {
                    item(key = "own-empty") {
                        Text(
                            stringResource(R.string.own_empty),
                            Modifier.padding(start = 16.dp, end = 16.dp, bottom = 8.dp),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
                items(own, key = { it.id }) { e ->
                    OwnRow(
                        e,
                        onOpen = { onOpen(e.id) },
                        onRename = { renaming = e },
                        onExport = {
                            exporting = e
                            export.launch(e.name + ".pgn")
                        },
                        onDelete = {
                            if ((e.games ?: 0) == 0) {
                                val removed = library.remove(e.id) ?: return@OwnRow
                                scope.launch {
                                    snackbar.currentSnackbarData?.dismiss()
                                    var restored = false
                                    try {
                                        if (snackbar.showSnackbar(removedOwnText, undoText, withDismissAction = false, duration = SnackbarDuration.Long) == SnackbarResult.ActionPerformed) {
                                            library.restore(removed.first, removed.second)
                                            restored = true
                                        }
                                    } finally {
                                        if (!restored) library.forget(removed.second)
                                    }
                                }
                            } else {
                                deleting = e
                            }
                        },
                        modifier = Modifier.animateItem(),
                    )
                }
                item(key = "files-head") {
                    Text(
                        stringResource(R.string.files),
                        Modifier.fillMaxWidth().padding(start = 16.dp, top = 16.dp, bottom = 8.dp),
                        style = MaterialTheme.typography.titleSmall,
                        color = MaterialTheme.colorScheme.primary,
                    )
                }
                items(files, key = { it.id }) { e ->
                    DatabaseRow(
                        e,
                        download = views[e.id],
                        onOpen = { onOpen(e.id) },
                        onRename = { renaming = e },
                        onUpdate = { downloads.update(e) },
                        onStop = { downloads.cancel(e.id) },
                        onRemove = {
                            // A download into it stops first: its file goes with it.
                            downloads.cancel(e.id)
                            val removed = library.remove(e.id) ?: return@DatabaseRow
                            scope.launch {
                                snackbar.currentSnackbarData?.dismiss()
                                var restored = false
                                try {
                                    // A copy is deleted, not only taken off the list.
                                    val text = when (e.source) {
                                        Source.Copy -> removedCopyText
                                        Source.Online -> removedOnlineText
                                        else -> removedText
                                    }
                                    // Not for ever: the deleted file goes once "Undo" is gone.
                                    if (snackbar.showSnackbar(text, undoText, withDismissAction = false, duration = SnackbarDuration.Long) == SnackbarResult.ActionPerformed) {
                                        library.restore(removed.first, removed.second)
                                        restored = true
                                    }
                                } finally {
                                    // Also when the screen goes away while "Undo" is offered.
                                    if (!restored) library.forget(removed.second)
                                }
                            }
                        },
                        modifier = Modifier.animateItem(),
                    )
                }
            }
        }
    }
    val pasteNone = stringResource(R.string.paste_none)
    val resources = LocalResources.current
    if (choosing) {
        val clipboard = remember { context.getSystemService(ClipboardManager::class.java) }
        val hasText = remember { clipboard.primaryClipDescription?.hasMimeType("text/*") == true }
        AddSheet(
            onFile = {
                choosing = false
                pick.launch(arrayOf("*/*"))
            },
            onFolder = {
                choosing = false
                pickFolder.launch(null)
            },
            onPaste = if (!hasText) null else {
                {
                    choosing = false
                    val text = clipboard.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.toString().orEmpty()
                    val dated = resources.getString(
                        R.string.paste_db,
                        DateUtils.formatDateTime(context, System.currentTimeMillis(), DateUtils.FORMAT_SHOW_DATE or DateUtils.FORMAT_SHOW_TIME or DateUtils.FORMAT_ABBREV_MONTH),
                    )
                    scope.launch {
                        runCatching {
                            library.addCopy("clipboard.pgn", text.byteInputStream()) { db -> pastedName(db, dated) }
                        }.onSuccess(onAdded).onFailure {
                            snackbar.currentSnackbarData?.dismiss()
                            snackbar.showSnackbar(pasteNone)
                        }
                    }
                }
            },
            onOnline = {
                choosing = false
                onOnline()
            },
            onDismiss = { choosing = false },
        )
    }
    if (folderOpen) {
        FolderSheet(
            scan,
            onAdd = { chosen ->
                val s = scan ?: return@FolderSheet
                library.addFound(s.tree, chosen)
                folderOpen = false
                val n = chosen.count { !it.added && !it.incomplete }
                scope.launch {
                    snackbar.currentSnackbarData?.dismiss()
                    snackbar.showSnackbar(resources.getQuantityString(R.plurals.added_count, n, n))
                }
                // Nothing added: the folder's permission goes back.
                if (n == 0) library.forgetFolder(s.tree)
            },
            onPickAnother = {
                folderOpen = false
                folder?.let { library.forgetFolder(it) }
                pickFolder.launch(null)
            },
            onDismiss = {
                folderOpen = false
                folder?.let { library.forgetFolder(it) }
            },
        )
    }
    renaming?.let { e ->
        RenameDialog(displayName(e), own = e.source == Source.Own, onDismiss = { renaming = null }, onSave = {
            library.rename(e.id, it)
            renaming = null
        })
    }
    if (creating) {
        NewOwnDialog(
            suggested = freeName(stringResource(R.string.own_default_name), own.map { it.name }),
            onDismiss = { creating = false },
            onCreate = { name ->
                creating = false
                scope.launch { runCatching { library.createOwn(name) } }
            },
        )
    }
    deleting?.let { e ->
        val n = e.games ?: 0
        AlertDialog(
            onDismissRequest = { deleting = null },
            title = { Text(stringResource(R.string.own_delete_title, displayName(e))) },
            text = { Text(pluralStringResource(R.plurals.own_delete_text, n, groupDigits(n))) },
            confirmButton = {
                TextButton(onClick = {
                    deleting = null
                    library.remove(e.id)?.let { library.forget(it.second) }
                }) { Text(stringResource(R.string.delete), color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { deleting = null }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}

/// "My games", or "My games 2" when that one is taken.
fun freeName(base: String, taken: List<String>): String {
    if (base !in taken) return base
    var i = 2
    while ("$base $i" in taken) i++
    return "$base $i"
}

@Composable
private fun NewOwnDialog(suggested: String, onDismiss: () -> Unit, onCreate: (String) -> Unit) {
    var value by remember { mutableStateOf(TextFieldValue(suggested, TextRange(0, suggested.length))) }
    val trimmed = value.text.trim()
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.own_new_title)) },
        text = {
            OutlinedTextField(
                value = value,
                onValueChange = { value = it },
                singleLine = true,
                label = { Text(stringResource(R.string.own_name)) },
                supportingText = { Text(stringResource(R.string.own_new_hint)) },
            )
        },
        confirmButton = { TextButton(onClick = { onCreate(trimmed) }, enabled = trimmed.isNotEmpty()) { Text(stringResource(R.string.create)) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

/// An own database: its games are the user's, kept on the phone (android-play.md §30.2).
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun OwnRow(e: DbEntry, onOpen: () -> Unit, onRename: () -> Unit, onExport: () -> Unit, onDelete: () -> Unit, modifier: Modifier) {
    var menu by remember { mutableStateOf(false) }
    val n = e.games ?: 0
    Box(modifier) {
        Row(
            Modifier
                .fillMaxWidth()
                .height(72.dp)
                .combinedClickable(onClick = onOpen, onLongClick = { menu = true })
                .padding(start = 16.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                Modifier.size(40.dp).clip(RoundedCornerShape(8.dp)).background(MaterialTheme.colorScheme.primaryContainer),
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.Rounded.EditNote, null, tint = MaterialTheme.colorScheme.onPrimaryContainer) }
            Column(Modifier.weight(1f).padding(start = 16.dp)) {
                Text(displayName(e), style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    pluralStringResource(R.plurals.games_count, n, groupDigits(n)) + " · " + stringResource(R.string.own_on_phone),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Box {
                IconButton(onClick = { menu = true }) { Icon(Icons.Rounded.MoreVert, stringResource(R.string.menu)) }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(text = { Text(stringResource(R.string.rename)) }, onClick = {
                        menu = false
                        onRename()
                    })
                    DropdownMenuItem(text = { Text(stringResource(R.string.own_export)) }, onClick = {
                        menu = false
                        onExport()
                    })
                    DropdownMenuItem(text = { Text(stringResource(R.string.own_delete)) }, onClick = {
                        menu = false
                        onDelete()
                    })
                }
            }
        }
    }
}

@Composable
private fun Empty(modifier: Modifier, onAdd: () -> Unit, onSamples: () -> Unit) {
    Column(
        modifier.fillMaxSize().padding(horizontal = 32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Image(painterResource(R.drawable.penguin), null, Modifier.size(96.dp))
        Spacer(Modifier.height(24.dp))
        Text(stringResource(R.string.empty_title), style = MaterialTheme.typography.titleLarge, textAlign = TextAlign.Center)
        Spacer(Modifier.height(12.dp))
        Text(
            stringResource(R.string.empty_text),
            style = MaterialTheme.typography.bodyLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
            modifier = Modifier.widthIn(max = 296.dp),
        )
        Spacer(Modifier.height(32.dp))
        Button(onClick = onAdd, modifier = Modifier.height(48.dp)) {
            Icon(Icons.Rounded.Add, null)
            Spacer(Modifier.size(8.dp))
            Text(stringResource(R.string.add_database))
        }
        TextButton(onClick = onSamples, modifier = Modifier.height(48.dp)) { Text(stringResource(R.string.open_samples)) }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun DatabaseRow(
    e: DbEntry,
    download: DownloadView?,
    onOpen: () -> Unit,
    onRename: () -> Unit,
    onUpdate: () -> Unit,
    onStop: () -> Unit,
    onRemove: () -> Unit,
    modifier: Modifier,
) {
    var menu by remember { mutableStateOf(false) }
    val format = badge(e)
    val running = download?.state?.running == true
    val failure = download?.state?.failure?.takeIf { !running }
    val subtitle = e.games?.let {
        stringResource(R.string.games_with_format, pluralStringResource(R.plurals.games_count, it, groupDigits(it)), e.format ?: format)
    } ?: stringResource(R.string.not_opened, formatName(e))
    val shownSubtitle = when {
        // A copy kept in the app says so: removing it deletes it.
        e.source == Source.Copy -> subtitle + " · " + stringResource(R.string.db_copy)
        e.source != Source.Online -> subtitle
        // A downloaded database: how its download goes, or when it was last brought up to date.
        running -> stringResource(R.string.online_downloading, groupDigits(((e.games ?: 0) + download.state.added.toInt())))
        else -> pluralStringResource(R.plurals.games_count, e.games ?: 0, groupDigits(e.games ?: 0)) + " · " + when {
            // The games stay in the line: the database is not empty because a site failed.
            failure != null -> onlineProblemShort(failure)
            e.updated == null -> stringResource(R.string.online_unfinished)
            System.currentTimeMillis() - e.updated * 1000 < 60_000 -> stringResource(R.string.online_updated_now)
            else -> stringResource(
                R.string.online_updated,
                DateUtils.getRelativeTimeSpanString(e.updated * 1000, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS).toString(),
            )
        }
    }
    Box(modifier) {
        Row(
            Modifier
                .fillMaxWidth()
                .height(72.dp)
                .combinedClickable(onClick = onOpen, onLongClick = { menu = true })
                .padding(start = 16.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            FormatBadge(format)
            Column(Modifier.weight(1f).padding(start = 16.dp)) {
                Text(displayName(e), style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    shownSubtitle,
                    style = MaterialTheme.typography.bodyMedium,
                    color = if (failure != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Box {
                IconButton(onClick = { menu = true }) { Icon(Icons.Rounded.MoreVert, stringResource(R.string.menu)) }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    if (e.source == Source.Online) {
                        DropdownMenuItem(text = { Text(stringResource(if (running) R.string.online_stop_download else R.string.online_update)) }, onClick = {
                            menu = false
                            if (running) onStop() else onUpdate()
                        })
                    }
                    DropdownMenuItem(text = { Text(stringResource(R.string.rename)) }, onClick = {
                        menu = false
                        onRename()
                    })
                    DropdownMenuItem(text = { Text(stringResource(R.string.remove)) }, onClick = {
                        menu = false
                        onRemove()
                    })
                }
            }
        }
        // A download going on: a thin bar along the bottom of the row (§23.3).
        if (running) {
            val share = downloadShare(download)
            val bar = Modifier.fillMaxWidth().height(2.dp).align(Alignment.BottomStart)
            if (share != null) LinearProgressIndicator(progress = { share }, modifier = bar) else LinearProgressIndicator(bar)
        }
    }
}

@Composable
private fun RenameDialog(name: String, own: Boolean = false, onDismiss: () -> Unit, onSave: (String) -> Unit) {
    var value by remember { mutableStateOf(TextFieldValue(name, TextRange(0, name.length))) }
    val trimmed = value.text.trim()
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.rename)) },
        text = {
            OutlinedTextField(
                value = value,
                onValueChange = { value = it },
                singleLine = true,
                // An own database's file is ours: no "the file does not change".
                supportingText = if (own) null else { { Text(stringResource(R.string.rename_hint)) } },
            )
        },
        confirmButton = { TextButton(onClick = { onSave(trimmed) }, enabled = trimmed.isNotEmpty()) { Text(stringResource(R.string.save)) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

/// The name of pasted games: one game by its players, games of one event by it, others by
/// when they were pasted (`dated`), so that two pastes differ.
private fun pastedName(db: Database, dated: String): String {
    val rows = db.rowsOf((0u until db.len()).toList())
    if (rows.size == 1) {
        val (white, black) = shortName(rows[0].white) to shortName(rows[0].black)
        if (white != "?" || black != "?") return "$white – $black"
    } else {
        rows.map { it.event.trim() }.distinct().singleOrNull()?.takeIf { it.isNotEmpty() && it != "?" }?.let { return it }
    }
    return dated
}

/// The snackbar of a finished download: "“Name”: +12, 1 246 games in all", "“Name”: no
/// new games", "“Name”: Lichess does not answer"; none when it was stopped or got nothing.
fun finishedText(res: android.content.res.Resources, f: Finished): String? {
    val s = f.view.state
    val games = res.getQuantityString(R.plurals.games_count, f.games, groupDigits(f.games))
    s.failure?.let { return res.getString(R.string.online_snack_failed, f.name, problemShort(res, it)) }
    return when {
        s.cancelled || f.games == 0 -> null
        s.added == 0UL -> res.getString(R.string.online_snack_none, f.name)
        else -> res.getString(R.string.online_snack_added, f.name, groupDigits(s.added.toInt()), games)
    }
}
