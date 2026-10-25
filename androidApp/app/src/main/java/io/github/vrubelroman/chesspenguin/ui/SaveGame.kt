package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.DbEntry
import uniffi.mobile_api.GameData
import uniffi.mobile_api.GameSession

/// The game of a game screen as it is being changed (android-play.md §29–30), kept while
/// the phone turns: the screen is made again, the moves stay.
class GameEdit : ViewModel() {
    /// Which game: "database/index", or [NEW_GAME].
    var key: String? = null
    val session = mutableStateOf<GameSession?>(null)
    val data = mutableStateOf<GameData?>(null)
    /// What reading the game noticed: kept when the game is changed.
    var notes: List<String> = emptyList()
    /// Where a new game or a copy was saved: saving again replaces it there.
    var savedTo: Pair<String, UInt>? = null
    /// An opening being learned over the game (android-play.md §32), how far, the board's side.
    val trainer = mutableStateOf<uniffi.mobile_api.Trainer?>(null)
    var trainPly = 0
    var trainFlipped = false

    override fun onCleared() {
        trainer.value?.close()
        session.value?.close()
    }
}

/// The key of a new game (a game screen without a database).
const val NEW_GAME = "new"

/// «Save to a database» (android-play.md §30.6): the own databases to choose from (the
/// last one used first), or a new one named here.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SaveSheet(own: List<DbEntry>, last: String?, onSave: (DbEntry) -> Unit, onCreate: (String) -> Unit, onDismiss: () -> Unit) {
    var chosen by remember { mutableStateOf(own.firstOrNull { it.id == last }?.id ?: own.firstOrNull()?.id) }
    var creating by remember { mutableStateOf(own.isEmpty()) }
    val defaultName = stringResource(R.string.own_default_name)
    var name by remember { mutableStateOf(defaultName) }
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.fillMaxWidth().padding(bottom = 24.dp)) {
            Text(stringResource(R.string.save_to_db), style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp))
            Column(Modifier.selectableGroup()) {
                for (e in own) {
                    val n = e.games ?: 0
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .height(56.dp)
                            .selectable(selected = !creating && chosen == e.id, role = Role.RadioButton) {
                                creating = false
                                chosen = e.id
                            }
                            .padding(horizontal = 16.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        RadioButton(selected = !creating && chosen == e.id, onClick = null)
                        Text(displayName(e), Modifier.weight(1f).padding(start = 16.dp), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(pluralStringResource(R.plurals.games_count, n, groupDigits(n)), color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)
                    }
                }
            }
            if (own.isNotEmpty() && !creating) {
                Row(
                    Modifier.fillMaxWidth().height(56.dp).clickable { creating = true }.padding(horizontal = 24.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(Icons.Rounded.Add, null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text(stringResource(R.string.own_new_ellipsis), Modifier.padding(start = 16.dp))
                }
            }
            if (creating) {
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    singleLine = true,
                    label = { Text(stringResource(R.string.own_name)) },
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 8.dp),
                )
            }
            Spacer(Modifier.size(8.dp))
            Row(Modifier.fillMaxWidth().padding(horizontal = 24.dp), horizontalArrangement = Arrangement.End) {
                if (creating) {
                    Button(onClick = { onCreate(name.trim()) }, enabled = name.isNotBlank()) { Text(stringResource(R.string.create_and_save)) }
                } else {
                    Button(onClick = { own.firstOrNull { it.id == chosen }?.let(onSave) }, enabled = chosen != null) { Text(stringResource(R.string.save)) }
                }
            }
        }
    }
}
