package io.github.vrubelroman.chesspenguin.data

import android.content.Context
import io.github.vrubelroman.chesspenguin.R
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/// A game written into its player's database: the database, its number there (from 0),
/// and whether the database was made for it.
data class Written(val id: String, val name: String, val index: UInt, val created: Boolean)

/// The players' databases (docs/design/player-games.md §6, decisions «Партии игрока
/// записываются сами»): every game against Maia goes by itself into an own database of
/// its player — «Roman — games against the computer», made at their first game; without
/// a name, «Games against the computer». Only new games are added; replacing and
/// deleting stay the user's.
class PlayerGames(private val context: Context, private val library: Library) {
    private val store = context.getSharedPreferences("players", Context.MODE_PRIVATE)
    private val lock = Mutex()

    /// The name as it is kept: trimmed, inner spaces single.
    fun tidy(name: String): String = name.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }.joinToString(" ")

    /// Names that are the same player: «Иван» and «иван ».
    private fun key(name: String) = tidy(name).lowercase()

    /// The name of a new database for the player.
    fun dbName(name: String): String = tidy(name).let {
        if (it.isEmpty()) context.getString(R.string.player_db_shared) else context.getString(R.string.player_db_name, it)
    }

    /// The player's database, while it is on the list.
    fun database(name: String): DbEntry? =
        store.getString("db:" + key(name), null)?.let { library.entry(it) }?.takeIf { it.source == Source.Own }

    /// Whether `id` is a player's database (a knight on its row).
    fun isPlayers(id: String): Boolean = store.all.any { (k, v) -> k.startsWith("db:") && v == id }

    /// The players who played here, the last first (named ones).
    fun players(): List<String> =
        (store.getString("order", "") ?: "").split('\n').filter { it.isNotEmpty() }
            .mapNotNull { store.getString("name:$it", null) }

    /// The player who starts a game goes first in the list.
    fun remember(name: String) {
        val k = key(name)
        if (k.isEmpty()) return
        val order = listOf(k) + (store.getString("order", "") ?: "").split('\n').filter { it.isNotEmpty() && it != k }
        store.edit().putString("name:$k", tidy(name)).putString("order", order.joinToString("\n")).apply()
    }

    /// The game in hand let go unfinished (× on its card, a new game in its place, Maia
    /// failing): kept in the player's database with `*` once both have moved, unless it
    /// was written already or the user turned writing off. What is in hand is taken at
    /// once (the caller may put a new game there right after); `then` on `scope`.
    /// Whether it is written (a game without moves of both sides is not).
    fun keepUnfinished(games: MaiaGames, record: Boolean, scope: CoroutineScope, then: (Written) -> Unit = {}): Boolean {
        if (!wouldKeep(games, record)) return false
        val m = games.current.value ?: return false
        val pgn = games.pgn() ?: return false
        scope.launch { runCatching { writeUnfinished(m, pgn) }.getOrNull()?.let(then) }
        return true
    }

    /// Whether the game in hand, let go now, would be kept.
    fun wouldKeep(games: MaiaGames, record: Boolean): Boolean {
        val m = games.current.value ?: return false
        if (m.over || !record || games.written() != null) return false
        val pgn = games.pgn() ?: return false
        return plies(pgn) - m.floor >= 2
    }

    /// Half-moves of the main line.
    private fun plies(pgn: String): Int {
        val s = uniffi.mobile_api.GameSession.fromPgn(pgn)
        try {
            val d = s.data()
            var n = 0
            var plies = 0
            while (true) {
                n = d.nodes[n].children.firstOrNull()?.toInt() ?: return plies
                plies++
            }
        } finally {
            s.close()
        }
    }

    private suspend fun writeUnfinished(m: PlayMeta, pgn: String): Written {
        val s = uniffi.mobile_api.GameSession.fromPgn(pgn)
        val text = try {
            s.setData(listOf(uniffi.mobile_api.Tag("Termination", "Unterminated")), "*")
            s.pgn()
        } finally {
            s.close()
        }
        return write(m.name, text)
    }

    /// Adds `pgn` to the player's database, making it first when they have none.
    suspend fun write(name: String, pgn: String): Written = lock.withLock {
        val existing = database(name)
        val entry = existing ?: library.createOwn(dbName(name)).also {
            store.edit().putString("db:" + key(name), it.id).apply()
        }
        val n = library.saveOwn(entry.id, null, pgn)
        Written(entry.id, entry.name, n, existing == null)
    }
}
