package io.github.vrubelroman.chesspenguin

import androidx.compose.runtime.Composable
import androidx.navigation3.runtime.NavKey
import androidx.navigation3.runtime.entryProvider
import androidx.navigation3.runtime.rememberNavBackStack
import androidx.navigation3.ui.NavDisplay
import io.github.vrubelroman.chesspenguin.ui.AboutScreen
import io.github.vrubelroman.chesspenguin.ui.BoardStyleScreen
import io.github.vrubelroman.chesspenguin.ui.DatabasesScreen
import io.github.vrubelroman.chesspenguin.ui.SettingsScreen
import io.github.vrubelroman.chesspenguin.ui.GameScreen
import io.github.vrubelroman.chesspenguin.ui.GamesScreen
import kotlinx.serialization.Serializable

@Serializable data object Databases : NavKey

@Serializable data class Games(val db: String) : NavKey

/// The game at `place` on the list of games found by `query` in the database (kept, so
/// the same game comes back after Android restarts the app).
@Serializable data class GameAt(val db: String, val place: Int, val query: String = "") : NavKey

@Serializable data object SettingsKey : NavKey

@Serializable data object AboutKey : NavKey

@Serializable data object BoardStyleKey : NavKey

@Composable
fun MainNavigation() {
    val backStack = rememberNavBackStack(Databases)
    NavDisplay(
        backStack = backStack,
        onBack = { backStack.removeLastOrNull() },
        entryProvider = entryProvider {
            entry<Databases> {
                DatabasesScreen(onOpen = { backStack.add(Games(it)) }, onSettings = { backStack.add(SettingsKey) })
            }
            entry<Games> { key ->
                GamesScreen(key.db, onBack = { backStack.removeLastOrNull() }, onGame = { place, query -> backStack.add(GameAt(key.db, place, query)) })
            }
            entry<GameAt> { key ->
                GameScreen(key.db, key.place, key.query, onBack = { backStack.removeLastOrNull() }, onSettings = { backStack.add(SettingsKey) })
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
            entry<AboutKey> {
                AboutScreen(onBack = { backStack.removeLastOrNull() })
            }
        },
    )
}
