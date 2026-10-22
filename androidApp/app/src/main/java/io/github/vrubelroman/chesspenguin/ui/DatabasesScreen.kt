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
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.MoreVert
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
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
import io.github.vrubelroman.chesspenguin.data.DbEntry
import io.github.vrubelroman.chesspenguin.data.FolderScan
import io.github.vrubelroman.chesspenguin.data.Source
import uniffi.mobile_api.Database
import kotlinx.coroutines.launch

/// The first screen: the databases on the list (android-ui.md §3).
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DatabasesScreen(onOpen: (String) -> Unit, onAdded: (DbEntry) -> Unit, onSettings: () -> Unit) {
    val library = app().library
    val entries by library.entries.collectAsState()
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val removedText = stringResource(R.string.removed)
    val removedCopyText = stringResource(R.string.removed_copy)
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
    val list = rememberLazyListState()
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
            Empty(Modifier.padding(padding), onAdd = add, onSamples = {
                library.addSamples()
                library.entries.value.firstOrNull()?.let { onOpen(it.id) }
            })
        } else {
            LazyColumn(
                Modifier.fillMaxSize(),
                state = list,
                contentPadding = PaddingValues(top = padding.calculateTopPadding(), bottom = padding.calculateBottomPadding() + 88.dp),
            ) {
                items(entries, key = { it.id }) { e ->
                    DatabaseRow(
                        e,
                        onOpen = { onOpen(e.id) },
                        onRename = { renaming = e },
                        onRemove = {
                            val removed = library.remove(e.id) ?: return@DatabaseRow
                            scope.launch {
                                snackbar.currentSnackbarData?.dismiss()
                                var restored = false
                                try {
                                    // A copy is deleted, not only taken off the list.
                                    val text = if (e.source == Source.Copy) removedCopyText else removedText
                                    if (snackbar.showSnackbar(text, undoText, withDismissAction = false) == SnackbarResult.ActionPerformed) {
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
    val context = androidx.compose.ui.platform.LocalContext.current
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
        RenameDialog(displayName(e), onDismiss = { renaming = null }, onSave = {
            library.rename(e.id, it)
            renaming = null
        })
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
private fun DatabaseRow(e: DbEntry, onOpen: () -> Unit, onRename: () -> Unit, onRemove: () -> Unit, modifier: Modifier) {
    var menu by remember { mutableStateOf(false) }
    val format = badge(e)
    val subtitle = e.games?.let {
        stringResource(R.string.games_with_format, pluralStringResource(R.plurals.games_count, it, groupDigits(it)), e.format ?: format)
    } ?: stringResource(R.string.not_opened, formatName(e))
    // A copy kept in the app says so: removing it deletes it.
    val shownSubtitle = if (e.source == Source.Copy) subtitle + " · " + stringResource(R.string.db_copy) else subtitle
    Row(
        modifier
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
                DropdownMenuItem(text = { Text(stringResource(R.string.remove)) }, onClick = {
                    menu = false
                    onRemove()
                })
            }
        }
    }
}

@Composable
private fun RenameDialog(name: String, onDismiss: () -> Unit, onSave: (String) -> Unit) {
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
                supportingText = { Text(stringResource(R.string.rename_hint)) },
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
