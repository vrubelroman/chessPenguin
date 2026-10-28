package io.github.vrubelroman.chesspenguin.data

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import uniffi.mobile_api.DownloadState
import uniffi.mobile_api.OnlineAccount
import uniffi.mobile_api.OnlineDownload
import uniffi.mobile_api.onlineDatabase
import java.io.File
import java.util.concurrent.ConcurrentHashMap

/// A download of a database and how it stands (or how it ended); `fresh` when the
/// database did not exist before it (its first download).
data class DownloadView(val accounts: List<OnlineAccount>, val state: DownloadState, val fresh: Boolean = false)

/// A download that has ended, for the snackbar of the screen that is shown then;
/// `quiet` when the app started it on its own (said only when it added games).
data class Finished(val id: String, val name: String, val view: DownloadView, val games: Int, val quiet: Boolean = false)

/// The downloads of games from Lichess and Chess.com (android-ui.md §23). They belong to
/// the app, not to a screen: leaving the screen does not stop them, and the list of
/// databases shows how they go.
class OnlineDownloads(private val library: Library, private val scope: CoroutineScope) {
    private val _views = MutableStateFlow<Map<String, DownloadView>>(emptyMap())
    /// By database id: the running downloads, and the last one of each database.
    val views: StateFlow<Map<String, DownloadView>> = _views.asStateFlow()

    private val running = ConcurrentHashMap<String, OnlineDownload>()

    private val _finished = MutableSharedFlow<Finished>(extraBufferCapacity = 8)
    /// Each download as it ends (android-ui.md §23.3: a snackbar says how it went).
    val finished: SharedFlow<Finished> = _finished.asSharedFlow()

    /// Where the downloaded databases are kept.
    val dir: File get() = library.onlineDir

    /// Starts fetching the new games of `accounts` into the database file `path` (put on
    /// the list at once); one download per database. Returns the database's entry.
    fun start(accounts: List<OnlineAccount>, path: String): DbEntry = run(accounts, path, false).first

    private fun run(accounts: List<OnlineAccount>, path: String, quiet: Boolean): Pair<DbEntry, Job?> {
        val fresh = !File(path).exists()
        val entry = library.addOnline(File(path))
        if (running.containsKey(entry.id)) return entry to null
        val d = OnlineDownload.start(accounts, path)
        running[entry.id] = d
        val job = scope.launch(Dispatchers.IO) {
            while (true) {
                val s = d.state()
                _views.update { it + (entry.id to DownloadView(accounts, s, fresh)) }
                if (!s.running) break
                delay(POLL_MS)
            }
            running.remove(entry.id)
            d.close()
            val info = onlineDatabase(path)
            val games = info?.games?.toInt() ?: 0
            // No game at all (no standard chess there, or it failed at once): not a database.
            val name = library.entry(entry.id)?.name ?: entry.name
            if (games == 0) library.dropEmpty(entry.id) else library.downloaded(entry.id, games, info?.updated)
            _views.value[entry.id]?.let { _finished.tryEmit(Finished(entry.id, name, it, games, quiet)) }
        }
        return entry to job
    }

    /// Fetches the games played since the last download of a database.
    fun update(entry: DbEntry) {
        val path = library.onlineFile(entry).path
        val info = onlineDatabase(path) ?: return
        start(info.accounts, path)
    }

    /// Every downloaded database gets the games played since, one after another (the
    /// sites ask for one export at a time): at the app's start and when it comes back to
    /// the screen, at most every [AUTO_EVERY_MS].
    fun updateAllQuietly() {
        val now = System.currentTimeMillis()
        if (auto?.isActive == true || now - lastAuto < AUTO_EVERY_MS) return
        lastAuto = now
        auto = scope.launch(Dispatchers.IO) {
            for (e in library.entries.value.filter { it.source == Source.Online }) {
                if (isRunning(e.id)) continue
                val path = library.onlineFile(e).path
                val info = onlineDatabase(path) ?: continue
                run(info.accounts, path, true).second?.join()
            }
        }
    }

    private var lastAuto = 0L
    private var auto: Job? = null

    /// Stops a download; what it fetched stays.
    fun cancel(id: String) {
        running[id]?.cancel()
    }

    fun isRunning(id: String): Boolean = running.containsKey(id)

    private companion object {
        const val POLL_MS = 400L
        const val AUTO_EVERY_MS = 10 * 60 * 1000L
    }
}
