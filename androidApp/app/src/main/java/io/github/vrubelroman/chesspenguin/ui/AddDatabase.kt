package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.ui.draw.clip
import androidx.compose.foundation.background
import io.github.vrubelroman.chesspenguin.theme.DbKind
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.CloudDownload
import androidx.compose.material.icons.outlined.ContentPaste
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.FolderScan
import io.github.vrubelroman.chesspenguin.data.Found
import io.github.vrubelroman.chesspenguin.data.Source

/// The icon of a way to add, in a round tile coloured as the database it brings
/// (android-lists.md §6).
@Composable
private fun KindIcon(kind: DbKind, icon: androidx.compose.ui.graphics.vector.ImageVector) {
    val c = io.github.vrubelroman.chesspenguin.theme.kindColor(kind, io.github.vrubelroman.chesspenguin.theme.LocalChessColors.current.dark)
    Box(Modifier.size(40.dp).clip(androidx.compose.foundation.shape.CircleShape).background(c.container), contentAlignment = Alignment.Center) {
        Icon(icon, null, tint = c.content, modifier = Modifier.size(22.dp))
    }
}

/// "Add a database": games in the clipboard, a file, every database in a folder, or a
/// player's games from Lichess and Chess.com.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AddSheet(onFile: () -> Unit, onFolder: () -> Unit, onPaste: (() -> Unit)?, onOnline: () -> Unit, onDismiss: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.navigationBarsPadding().padding(bottom = 24.dp)) {
            Text(
                stringResource(R.string.add_database),
                style = MaterialTheme.typography.titleLarge,
                modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp).semantics { heading() },
            )
            // Only when there is text to paste (asked without reading it: reading the
            // clipboard makes Android say so on the screen).
            if (onPaste != null) {
                ListItem(
                    headlineContent = { Text(stringResource(R.string.add_paste)) },
                    supportingContent = { Text(stringResource(R.string.add_paste_hint)) },
                    leadingContent = { KindIcon(DbKind.Pgn, Icons.Outlined.ContentPaste) },
                    colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                    modifier = Modifier.clickable(role = Role.Button, onClick = onPaste).padding(horizontal = 8.dp),
                )
            }
            ListItem(
                headlineContent = { Text(stringResource(R.string.online_title)) },
                supportingContent = { Text(stringResource(R.string.add_online_hint)) },
                leadingContent = { KindIcon(DbKind.Online, Icons.Outlined.CloudDownload) },
                colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                modifier = Modifier.clickable(role = Role.Button, onClick = onOnline).padding(horizontal = 8.dp),
            )
            ListItem(
                headlineContent = { Text(stringResource(R.string.add_file)) },
                supportingContent = { Text(stringResource(R.string.add_file_hint)) },
                leadingContent = { KindIcon(DbKind.Pgn, Icons.Outlined.Description) },
                colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                modifier = Modifier.clickable(role = Role.Button, onClick = onFile).padding(horizontal = 8.dp),
            )
            ListItem(
                headlineContent = { Text(stringResource(R.string.add_folder)) },
                supportingContent = { Text(stringResource(R.string.add_folder_hint)) },
                leadingContent = { KindIcon(DbKind.Pgn, Icons.Outlined.Folder) },
                colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                modifier = Modifier.clickable(role = Role.Button, onClick = onFolder).padding(horizontal = 8.dp),
            )
            // The system picker only says "Can't use this folder" in English, and gives
            // nothing back: so it is said before.
            Text(
                stringResource(R.string.add_tree_hint),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp),
            )
        }
    }
}

/// The databases found in a picked folder, to tick and add; `scan` is null while the
/// folder is being looked through.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FolderSheet(scan: FolderScan?, onAdd: (List<Found>) -> Unit, onPickAnother: () -> Unit, onDismiss: () -> Unit) {
    // Open in full at once: the buttons under the list are always on the screen.
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.navigationBarsPadding().padding(horizontal = 24.dp).padding(bottom = 24.dp)) {
            if (scan == null) {
                Row(Modifier.fillMaxWidth().height(96.dp), verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 3.dp)
                    Spacer(Modifier.size(16.dp))
                    Text(stringResource(R.string.folder_scanning), style = MaterialTheme.typography.bodyLarge)
                }
                return@Column
            }
            val addable = scan.found.filter { !it.added && !it.incomplete }
            if (scan.found.isEmpty()) {
                Text(stringResource(R.string.folder_empty_title), style = MaterialTheme.typography.titleLarge, modifier = Modifier.semantics { heading() })
                Spacer(Modifier.height(8.dp))
                Text(
                    stringResource(R.string.folder_empty_text),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(24.dp))
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                    TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) }
                    Button(onClick = onPickAnother) { Text(stringResource(R.string.pick_another)) }
                }
                return@Column
            }
            // Everything that can be added is ticked at first.
            var chosen by remember(scan) { mutableStateOf(addable.toSet()) }
            Text(
                pluralStringResource(R.plurals.folder_found, scan.found.size, scan.found.size, scan.name),
                style = MaterialTheme.typography.titleLarge,
                modifier = Modifier.semantics { heading() },
            )
            if (addable.size > 1) {
                val all = chosen.size == addable.size
                TextButton(onClick = { chosen = if (all) emptySet() else addable.toSet() }, contentPadding = PaddingValues(0.dp)) {
                    Text(stringResource(if (all) R.string.choose_none else R.string.choose_all))
                }
            }
            Spacer(Modifier.height(8.dp))
            LazyColumn(Modifier.weight(1f, fill = false)) {
                itemsIndexed(scan.found) { _, f ->
                    FoundRow(f, f in chosen) { on -> chosen = if (on) chosen + f else chosen - f }
                }
            }
            Spacer(Modifier.height(16.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                if (addable.isEmpty()) {
                    // Everything there is already on the list (or cannot be opened).
                    Button(onClick = onDismiss) { Text(stringResource(R.string.done)) }
                } else {
                    TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) }
                    Button(onClick = { onAdd(scan.found.filter { it in chosen }) }, enabled = chosen.isNotEmpty()) {
                        Text(if (chosen.isEmpty()) stringResource(R.string.add) else pluralStringResource(R.plurals.add_chosen, chosen.size, chosen.size))
                    }
                }
            }
        }
    }
}

@Composable
private fun FoundRow(f: Found, checked: Boolean, onChange: (Boolean) -> Unit) {
    val enabled = !f.added && !f.incomplete
    val label = when {
        f.source == Source.ChessBase -> "CB"
        else -> f.fileName.substringAfterLast('.', "").uppercase().let { if (it == "CBV") "CB" else it }
    }
    val note = when {
        f.added -> stringResource(R.string.folder_added)
        f.incomplete -> stringResource(R.string.folder_incomplete)
        else -> f.fileName
    }
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 64.dp)
            .toggleable(value = checked || f.added, enabled = enabled, role = Role.Checkbox, onValueChange = onChange),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Checkbox(checked = checked || f.added, onCheckedChange = null, enabled = enabled)
        Spacer(Modifier.size(12.dp))
        FormatBadge(label, error = f.incomplete, kind = when (label) { "CB" -> DbKind.ChessBase; "7Z", "ZIP" -> DbKind.Archive; else -> DbKind.Pgn })
        Column(Modifier.weight(1f).padding(start = 16.dp)) {
            Text(f.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val where = if (f.path.isEmpty()) note else "${f.path} · $note"
            Text(
                where,
                style = MaterialTheme.typography.bodyMedium,
                color = if (f.incomplete) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Box(Modifier.size(8.dp))
    }
}
