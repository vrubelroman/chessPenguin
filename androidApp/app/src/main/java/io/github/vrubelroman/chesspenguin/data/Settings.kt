package io.github.vrubelroman.chesspenguin.data

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

enum class Theme { System, Light, Dark }

/// Who plays against the user (android-rodent.md): kept by these names.
enum class Opponent(val key: String) { Maia("maia"), Rodent("rodent4") }

/// How moves are written on the screen (PGN keeps K Q R B N).
enum class Notation { Latin, Figurines, Local }

/// The user's choices (android-ui.md §9), applied at once and kept across starts.
data class Prefs(
    val theme: Theme = Theme.Dark,
    val notation: Notation = Notation.Latin,
    val board: BoardTheme = BoardTheme.Green,
    val pieces: PieceSet = PieceSet.Merida,
    val vibration: Boolean = true,
    val animation: Boolean = true,
    val bestArrow: Boolean = true,
    val engineLines: Int = 2,
    /// Seconds of thinking per position; 0 = no limit.
    val engineSeconds: Int = 20,
    val engineThreads: Int = defaultThreads(),
    val engineHashMb: Int = 64,
    /// The game report's quality, an index into REVIEW_NODES (§21.2).
    val reviewQuality: Int = 1,
    /// The engine's speed per thread measured by the last game analysis (0 = not yet),
    /// for the time estimate.
    val reviewNps: Long = 0,
    /// The last choices of a game against the computer (android-play.md §31.1,
    /// android-rodent.md §7.1): the opponent, each one's strength on its own scale,
    /// Rodent's character ("" = never chosen), the user's colour (-1 Black, 0 at
    /// random, 1 White), the user's name.
    val opponent: Opponent = Opponent.Maia,
    val maiaStrength: Int = 1500,
    val rodentStrength: Int = 1500,
    val rodentPersonality: String = "",
    val playColor: Int = 0,
    val yourName: String = "",
    /// Sounds of moves and of the end of a game (§33).
    val sound: Boolean = true,
    /// Games against the computer go by themselves into the player's database (player-games.md).
    val recordGames: Boolean = true,
)

/// Half the cores, at most 2: the engine leaves the phone's interface room (§9).
fun defaultThreads(): Int = (Runtime.getRuntime().availableProcessors() / 2).coerceIn(1, 2)

class Settings(context: Context) {
    private val store = context.getSharedPreferences("settings", Context.MODE_PRIVATE)
    private val _prefs = MutableStateFlow(read())

    init {
        // Up to 0.11 the engine button was kept here (it is not remembered any more), and
        // the threads were half the cores up to 4: that default becomes the new one.
        if (store.getInt("version", 1) < 2) {
            val oldDefault = (Runtime.getRuntime().availableProcessors() / 2).coerceIn(1, 4)
            if (store.getInt("engineThreads", oldDefault) == oldDefault) update { it.copy(engineThreads = defaultThreads()) }
            store.edit().remove("engineOn").putInt("version", 2).apply()
        }
    }
    val prefs: StateFlow<Prefs> = _prefs.asStateFlow()

    private fun read(): Prefs {
        val d = Prefs()
        return Prefs(
            theme = enumValueOrNull<Theme>(store.getString("theme", null)) ?: d.theme,
            notation = enumValueOrNull<Notation>(store.getString("notation", null)) ?: d.notation,
            board = enumValueOrNull<BoardTheme>(store.getString("board", null)) ?: d.board,
            pieces = enumValueOrNull<PieceSet>(store.getString("pieces", null)) ?: d.pieces,
            vibration = store.getBoolean("vibration", d.vibration),
            animation = store.getBoolean("animation", d.animation),
            bestArrow = store.getBoolean("bestArrow", d.bestArrow),
            engineLines = store.getInt("engineLines", d.engineLines),
            engineSeconds = store.getInt("engineSeconds", d.engineSeconds),
            engineThreads = store.getInt("engineThreads", d.engineThreads),
            engineHashMb = store.getInt("engineHashMb", d.engineHashMb),
            reviewQuality = store.getInt("reviewQuality", d.reviewQuality).coerceIn(0, REVIEW_NODES.size - 1),
            reviewNps = store.getLong("reviewNps", d.reviewNps),
            opponent = Opponent.entries.firstOrNull { it.key == store.getString("opponent", null) } ?: d.opponent,
            maiaStrength = store.getInt("maiaStrength", d.maiaStrength),
            rodentStrength = store.getInt("rodentStrength", d.rodentStrength),
            rodentPersonality = store.getString("rodentPersonality", d.rodentPersonality) ?: d.rodentPersonality,
            // Kept as `maiaColor` up to 1.7, when Maia was the only opponent.
            playColor = store.getInt("playColor", store.getInt("maiaColor", d.playColor)).coerceIn(-1, 1),
            yourName = store.getString("yourName", d.yourName) ?: d.yourName,
            sound = store.getBoolean("sound", d.sound),
            recordGames = store.getBoolean("recordGames", d.recordGames),
        )
    }

    fun update(change: (Prefs) -> Prefs) {
        val p = change(_prefs.value)
        _prefs.value = p
        store.edit()
            .putString("theme", p.theme.name)
            .putString("notation", p.notation.name)
            .putString("board", p.board.name)
            .putString("pieces", p.pieces.name)
            .putBoolean("vibration", p.vibration)
            .putBoolean("animation", p.animation)
            .putBoolean("bestArrow", p.bestArrow)
            .putInt("engineLines", p.engineLines)
            .putInt("engineSeconds", p.engineSeconds)
            .putInt("engineThreads", p.engineThreads)
            .putInt("engineHashMb", p.engineHashMb)
            .putInt("reviewQuality", p.reviewQuality)
            .putLong("reviewNps", p.reviewNps)
            .putString("opponent", p.opponent.key)
            .putInt("maiaStrength", p.maiaStrength)
            .putInt("rodentStrength", p.rodentStrength)
            .putString("rodentPersonality", p.rodentPersonality)
            .putInt("playColor", p.playColor)
            .remove("maiaColor")
            .putString("yourName", p.yourName)
            .putBoolean("sound", p.sound)
            .putBoolean("recordGames", p.recordGames)
            .apply()
    }
}

private inline fun <reified T : Enum<T>> enumValueOrNull(name: String?): T? =
    enumValues<T>().firstOrNull { it.name == name }
