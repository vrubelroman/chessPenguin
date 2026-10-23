package io.github.vrubelroman.chesspenguin

import android.app.Application
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import android.net.Uri
import android.provider.OpenableColumns
import io.github.vrubelroman.chesspenguin.data.AppUpdates
import io.github.vrubelroman.chesspenguin.data.DbEntry
import io.github.vrubelroman.chesspenguin.data.EngineFiles
import io.github.vrubelroman.chesspenguin.data.Library
import io.github.vrubelroman.chesspenguin.data.OnlineDownloads
import io.github.vrubelroman.chesspenguin.data.Settings
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.util.concurrent.atomic.AtomicInteger
import java.io.FileNotFoundException
import java.util.UUID

/// The app's databases, settings and engine files live as long as the process.
class ChessPenguinApp : Application() {
    /// Work that outlives a screen (the download of the engine).
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    val library by lazy { Library(this) }
    val settings by lazy { Settings(this) }
    val engineFiles by lazy { EngineFiles(this, scope) }
    /// Games from Lichess and Chess.com being downloaded (android-ui.md §23).
    val downloads by lazy { OnlineDownloads(library, scope) }
    /// The app's own updates (android-ui.md §25).
    val updates by lazy { AppUpdates(this, scope, engineFiles) }

    /// The games shown on the list of each database (after a search), for paging
    /// games from the game screen: database id → game numbers.
    val shown = HashMap<String, List<UInt>>()

    /// Changes each time the app goes to the background, and in every new process: the
    /// engine button of a game holds only for the visit it was pressed in (§8.3).
    var visit by mutableStateOf(UUID.randomUUID().toString())
        private set

    fun leftApp() {
        visit = UUID.randomUUID().toString()
    }

    /// A file opened from another app: being copied in, on the list, or why not.
    val incoming = MutableStateFlow<Incoming?>(null)

    /// One copy at a time, in the order the files came; only the last one opened is shown.
    private val copying = Mutex()
    private val waiting = AtomicInteger()

    /// Copies a file another app gave (it gives access only for a while) and puts it on
    /// the list; `incoming` tells the screens how it went.
    fun receive(uri: Uri) {
        waiting.incrementAndGet()
        incoming.value = Incoming.Opening
        scope.launch {
            val result = copying.withLock {
                try {
                    val name = displayName(uri) ?: "games.pgn"
                    val ext = name.substringAfterLast('.', "").lowercase()
                    // One file of a ChessBase database: the others are not given with it.
                    if (ext in CHESSBASE_PARTS) throw ChessBasePart()
                    // A name with no known ending (some apps give "file_123") is read as PGN.
                    val fileName = if (ext in setOf("pgn", "zip", "7z", "cbv")) name else "$name.pgn"
                    val input = contentResolver.openInputStream(uri) ?: throw FileNotFoundException(uri.toString())
                    Incoming.Opened(input.use { library.addCopy(fileName, it) { name.substringBeforeLast('.') } })
                } catch (e: Exception) {
                    Incoming.Failed(e)
                }
            }
            val more = waiting.decrementAndGet() > 0
            // A file opened while another one was still being copied comes after it.
            if (!(more && result is Incoming.Opened)) incoming.value = result
        }
    }

    private fun displayName(uri: Uri): String? = runCatching {
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        }
    }.getOrNull()
}

/// A `.cbh` (or another part) of a ChessBase database, opened alone.
class ChessBasePart : Exception()

private val CHESSBASE_PARTS = setOf("cbh", "cbg", "cba", "cbp", "cbs", "cbj", "cbt", "cbc", "cbe", "cbm")

sealed interface Incoming {
    data object Opening : Incoming
    data class Opened(val entry: DbEntry) : Incoming
    data class Failed(val error: Throwable) : Incoming
}
