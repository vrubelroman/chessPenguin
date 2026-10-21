package io.github.vrubelroman.chesspenguin

import androidx.compose.runtime.Composable
import androidx.navigation3.runtime.NavKey
import androidx.navigation3.runtime.entryProvider
import androidx.navigation3.runtime.rememberNavBackStack
import androidx.navigation3.ui.NavDisplay
import io.github.vrubelroman.chesspenguin.ui.AboutScreen
import io.github.vrubelroman.chesspenguin.ui.DatabasesScreen
import io.github.vrubelroman.chesspenguin.ui.SettingsScreen
import io.github.vrubelroman.chesspenguin.ui.GameScreen
import io.github.vrubelroman.chesspenguin.ui.GamesScreen
import kotlinx.serialization.Serializable

@Serializable data object Databases : NavKey

@Serializable data class Games(val db: String) : NavKey

/// The game at `place` on the list of games shown for the database.
@Serializable data class GameAt(val db: String, val place: Int) : NavKey

@Serializable data object SettingsKey : NavKey

@Serializable data object AboutKey : NavKey

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
                GamesScreen(key.db, onBack = { backStack.removeLastOrNull() }, onGame = { backStack.add(GameAt(key.db, it)) })
            }
            entry<GameAt> { key ->
                GameScreen(key.db, key.place, onBack = { backStack.removeLastOrNull() }, onSettings = { backStack.add(SettingsKey) })
            }
            entry<SettingsKey> {
                SettingsScreen(onBack = { backStack.removeLastOrNull() }, onAbout = { backStack.add(AboutKey) })
            }
            entry<AboutKey> {
                AboutScreen(onBack = { backStack.removeLastOrNull() })
            }
        },
    )
}
