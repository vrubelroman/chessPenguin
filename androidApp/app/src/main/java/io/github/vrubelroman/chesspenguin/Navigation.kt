package io.github.vrubelroman.chesspenguin

import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.semantics.semantics
import io.github.vrubelroman.chesspenguin.data.DbEntry
import io.github.vrubelroman.chesspenguin.data.NoGames
import uniffi.mobile_api.ApiException
import java.io.FileNotFoundException
import io.github.vrubelroman.chesspenguin.ui.app
import io.github.vrubelroman.chesspenguin.ui.openProblem
import io.github.vrubelroman.chesspenguin.ui.UpdateSheet
import androidx.navigation3.runtime.NavKey
import androidx.navigation3.runtime.entryProvider
import androidx.navigation3.runtime.rememberSaveableStateHolderNavEntryDecorator
import androidx.lifecycle.viewmodel.navigation3.rememberViewModelStoreNavEntryDecorator
import androidx.navigation3.runtime.rememberNavBackStack
import androidx.navigation3.ui.NavDisplay
import io.github.vrubelroman.chesspenguin.ui.AboutScreen
import io.github.vrubelroman.chesspenguin.ui.BoardStyleScreen
import io.github.vrubelroman.chesspenguin.ui.DatabasesScreen
import io.github.vrubelroman.chesspenguin.ui.SettingsScreen
import io.github.vrubelroman.chesspenguin.ui.GameScreen
import io.github.vrubelroman.chesspenguin.ui.NEW_GAME
import io.github.vrubelroman.chesspenguin.ui.PLAY_GAME
import io.github.vrubelroman.chesspenguin.ui.GamesScreen
import io.github.vrubelroman.chesspenguin.ui.OnlineScreen
import kotlinx.serialization.Serializable

@Serializable data object Databases : NavKey

@Serializable data class Games(val db: String) : NavKey

/// The game at `place` on the list of games found by `query` in the database (kept, so
/// the same game comes back after Android restarts the app).
@Serializable data class GameAt(val db: String, val place: Int, val query: String = "") : NavKey

/// A new game from the start position (android-play.md §30.4).
@Serializable data object NewGameKey : NavKey

/// The game against Maia in hand (android-play.md §31).
@Serializable data object PlayKey : NavKey

@Serializable data object SettingsKey : NavKey

@Serializable data object AboutKey : NavKey

@Serializable data object BoardStyleKey : NavKey

@Serializable data object OnlineKey : NavKey

@Composable
fun MainNavigation() {
    val backStack = rememberNavBackStack(Databases)
    // A database just added from outside (the clipboard, another app): its list, or its
    // only game at once, over the list of databases.
    fun show(entry: DbEntry) {
        while (backStack.size > 1) backStack.removeAt(backStack.lastIndex)
        backStack.add(Games(entry.id))
        if (entry.games == 1) backStack.add(GameAt(entry.id, 0))
    }
    val app = app()
    val incoming by app.incoming.collectAsState()
    LaunchedEffect(incoming) {
        (incoming as? Incoming.Opened)?.let {
            app.incoming.value = null
            show(it.entry)
        }
    }
    // The app's update, over any screen (android-ui.md §25.3).
    val updateSheet by app.updates.sheet.collectAsState()
    if (updateSheet) UpdateSheet()
    when (val i = incoming) {
        Incoming.Opening -> Dialog(onDismissRequest = {}, properties = DialogProperties(dismissOnBackPress = false, dismissOnClickOutside = false)) {
            Surface(shape = MaterialTheme.shapes.extraLarge, color = MaterialTheme.colorScheme.surfaceContainerHigh) {
                Row(Modifier.padding(24.dp).semantics(mergeDescendants = true) {}, verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 3.dp)
                    Spacer(Modifier.size(16.dp))
                    Text(stringResource(R.string.opening_file), style = MaterialTheme.typography.bodyLarge)
                }
            }
        }
        is Incoming.Failed -> AlertDialog(
            onDismissRequest = { app.incoming.value = null },
            title = { Text(stringResource(R.string.file_failed)) },
            text = { Text(incomingProblem(i.error)) },
            confirmButton = { TextButton(onClick = { app.incoming.value = null }) { Text(stringResource(R.string.ok)) } },
        )
        else -> {}
    }
    NavDisplay(
        backStack = backStack,
        onBack = { backStack.removeLastOrNull() },
        // Each screen keeps its own saved state and view models: they go with the screen
        // when it leaves the stack (the search of the online screen must not outlive it).
        entryDecorators = listOf(
            rememberSaveableStateHolderNavEntryDecorator(),
            rememberViewModelStoreNavEntryDecorator(),
        ),
        entryProvider = entryProvider {
            entry<Databases> {
                DatabasesScreen(
                    onOpen = { backStack.add(Games(it)) },
                    onAdded = ::show,
                    onSettings = { backStack.add(SettingsKey) },
                    onOnline = { backStack.add(OnlineKey) },
                    onNewGame = { backStack.add(NewGameKey) },
                    onPlay = { backStack.add(PlayKey) },
                )
            }
            entry<NewGameKey> {
                GameScreen(NEW_GAME, 0, "", onBack = { backStack.removeLastOrNull() }, onSettings = { backStack.add(SettingsKey) }, onPlayGame = { backStack.add(PlayKey) })
            }
            entry<PlayKey> {
                GameScreen(PLAY_GAME, 0, "", onBack = { backStack.removeLastOrNull() }, onSettings = { backStack.add(SettingsKey) }, onPlayGame = {
                    backStack.removeLastOrNull()
                    backStack.add(PlayKey)
                })
            }
            entry<Games> { key ->
                GamesScreen(key.db, onBack = { backStack.removeLastOrNull() }, onGame = { place, query -> backStack.add(GameAt(key.db, place, query)) })
            }
            entry<GameAt> { key ->
                GameScreen(key.db, key.place, key.query, onBack = { backStack.removeLastOrNull() }, onSettings = { backStack.add(SettingsKey) }, onPlayGame = { backStack.add(PlayKey) })
            }
            entry<SettingsKey> {
                SettingsScreen(
                    onBack = { backStack.removeLastOrNull() },
                    onAbout = { backStack.add(AboutKey) },
                    onBoardStyle = { backStack.add(BoardStyleKey) },
                )
            }
            entry<BoardStyleKey> {
                BoardStyleScreen(onBack = { backStack.removeLastOrNull() })
            }
            entry<OnlineKey> {
                OnlineScreen(
                    onBack = { backStack.removeLastOrNull() },
                    // The downloaded database instead of this screen: "back" goes to the list.
                    onOpen = { id ->
                        backStack.removeLastOrNull()
                        backStack.add(Games(id))
                    },
                )
            }
            entry<AboutKey> {
                AboutScreen(onBack = { backStack.removeLastOrNull() })
            }
        },
    )
}

/// Why a file from another app did not open; it never got on the list, so the advice is
/// not that of a database on it (openProblem).
@Composable
private fun incomingProblem(e: Throwable): String = when (e) {
    is ChessBasePart -> stringResource(R.string.file_chessbase_part)
    is NoGames -> stringResource(R.string.file_no_games)
    is SecurityException, is FileNotFoundException, is ApiException.Io -> stringResource(R.string.file_unreadable)
    is ApiException -> openProblem(e).ifEmpty { stringResource(R.string.open_corrupt) }
    else -> stringResource(R.string.file_copy_failed)
}
