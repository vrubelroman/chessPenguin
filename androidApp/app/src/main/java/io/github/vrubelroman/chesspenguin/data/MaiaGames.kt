package io.github.vrubelroman.chesspenguin.data

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File

/// A game against Maia: her strength, the user's colour and name; `floor`: the half-moves
/// that came from another game ("Play on from here", not taken back); `over`: it ended.
data class PlayMeta(val strength: Int, val userWhite: Boolean, val name: String, val floor: Int, val over: Boolean = false)

/// The game against Maia being played (android-play.md §31.7): its PGN is written after
/// every move, so that "back" puts it aside and it outlives the app being closed. One at
/// a time.
class MaiaGames(context: Context) {
    private val file = File(context.filesDir, "play/current.pgn")
    private val prefs = context.getSharedPreferences("play", Context.MODE_PRIVATE)
    private val _current = MutableStateFlow(read())
    /// The game in hand, if any.
    val current: StateFlow<PlayMeta?> = _current.asStateFlow()

    private fun read(): PlayMeta? {
        if (!file.isFile || !prefs.contains("strength")) return null
        return PlayMeta(
            strength = prefs.getInt("strength", 1500),
            userWhite = prefs.getBoolean("white", true),
            name = prefs.getString("name", "") ?: "",
            floor = prefs.getInt("floor", 0),
            over = prefs.getBoolean("over", false),
        )
    }

    /// The PGN of the game in hand.
    fun pgn(): String? = runCatching { file.readText() }.getOrNull()

    /// A new game: it replaces the one in hand.
    fun begin(pgn: String, meta: PlayMeta) {
        write(pgn)
        prefs.edit()
            .putInt("strength", meta.strength)
            .putBoolean("white", meta.userWhite)
            .putString("name", meta.name)
            .putInt("floor", meta.floor)
            .putBoolean("over", meta.over)
            .apply()
        _current.value = meta
    }

    /// The game after a move.
    fun save(pgn: String) {
        if (_current.value != null) write(pgn)
    }

    /// The game ended: it is not offered to go on with any more.
    fun finish(pgn: String) {
        save(pgn)
        prefs.edit().putBoolean("over", true).apply()
        _current.value = _current.value?.copy(over = true)
    }

    /// The game is gone (left after its end, removed by the user).
    fun clear() {
        file.delete()
        prefs.edit().clear().apply()
        _current.value = null
    }

    /// Written beside and moved in: a cut-off write leaves the last move's game whole.
    private fun write(pgn: String) {
        file.parentFile?.mkdirs()
        val tmp = File(file.path + ".tmp")
        tmp.writeText(pgn)
        if (!tmp.renameTo(file)) {
            file.writeText(pgn)
            tmp.delete()
        }
    }
}
