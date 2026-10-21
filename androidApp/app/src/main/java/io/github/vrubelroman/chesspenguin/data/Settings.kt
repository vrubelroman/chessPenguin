package io.github.vrubelroman.chesspenguin.data

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

enum class Theme { System, Light, Dark }

/// How moves are written on the screen (PGN keeps K Q R B N).
enum class Notation { Latin, Figurines, Local }

/// The user's choices (android-ui.md §9), applied at once and kept across starts.
data class Prefs(
    val theme: Theme = Theme.System,
    val notation: Notation = Notation.Latin,
    val vibration: Boolean = true,
    val animation: Boolean = true,
    val bestArrow: Boolean = true,
    /// The engine button: once turned on, it stays on in the next game (§8.3).
    val engineOn: Boolean = false,
    val engineLines: Int = 2,
    /// Seconds of thinking per position; 0 = no limit.
    val engineSeconds: Int = 20,
    val engineThreads: Int = defaultThreads(),
    val engineHashMb: Int = 64,
)

/// Half the cores, at most 4 (§9).
fun defaultThreads(): Int = (Runtime.getRuntime().availableProcessors() / 2).coerceIn(1, 4)

class Settings(context: Context) {
    private val store = context.getSharedPreferences("settings", Context.MODE_PRIVATE)
    private val _prefs = MutableStateFlow(read())
    val prefs: StateFlow<Prefs> = _prefs.asStateFlow()

    private fun read(): Prefs {
        val d = Prefs()
        return Prefs(
            theme = enumValueOrNull<Theme>(store.getString("theme", null)) ?: d.theme,
            notation = enumValueOrNull<Notation>(store.getString("notation", null)) ?: d.notation,
            vibration = store.getBoolean("vibration", d.vibration),
            animation = store.getBoolean("animation", d.animation),
            bestArrow = store.getBoolean("bestArrow", d.bestArrow),
            engineOn = store.getBoolean("engineOn", d.engineOn),
            engineLines = store.getInt("engineLines", d.engineLines),
            engineSeconds = store.getInt("engineSeconds", d.engineSeconds),
            engineThreads = store.getInt("engineThreads", d.engineThreads),
            engineHashMb = store.getInt("engineHashMb", d.engineHashMb),
        )
    }

    fun update(change: (Prefs) -> Prefs) {
        val p = change(_prefs.value)
        _prefs.value = p
        store.edit()
            .putString("theme", p.theme.name)
            .putString("notation", p.notation.name)
            .putBoolean("vibration", p.vibration)
            .putBoolean("animation", p.animation)
            .putBoolean("bestArrow", p.bestArrow)
            .putBoolean("engineOn", p.engineOn)
            .putInt("engineLines", p.engineLines)
            .putInt("engineSeconds", p.engineSeconds)
            .putInt("engineThreads", p.engineThreads)
            .putInt("engineHashMb", p.engineHashMb)
            .apply()
    }
}

private inline fun <reified T : Enum<T>> enumValueOrNull(name: String?): T? =
    enumValues<T>().firstOrNull { it.name == name }
