package io.github.vrubelroman.chesspenguin.data

import android.os.SystemClock
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import uniffi.mobile_api.Analysis
import uniffi.mobile_api.Analyzer
import uniffi.mobile_api.GameData

/// The position for the engine: where the history starts and the moves since then.
data class EnginePosition(val startFen: String, val moves: List<String>)

/// The history of a node, so the engine sees repetitions; after a training null move
/// ("--") it starts again from the position as a FEN (android-ui.md §8, AGENTS.md).
fun enginePosition(game: GameData, node: Int): EnginePosition {
    val path = ArrayList<Int>()
    var n: Int? = node
    while (n != null) {
        path.add(n)
        n = game.nodes[n].parent?.toInt()
    }
    path.reverse()
    var start = game.nodes[0].fen
    val moves = ArrayList<String>()
    for (id in path.drop(1)) {
        val m = game.nodes[id]
        if (m.uci.isEmpty()) {
            start = m.fen
            moves.clear()
        } else {
            moves.add(m.uci)
        }
    }
    return EnginePosition(start, moves)
}

sealed interface EngineStatus {
    data object Off : EngineStatus
    data object Starting : EngineStatus
    /// Thinking about the current position.
    data object Thinking : EngineStatus
    /// The time per position is over; "Deeper" thinks on.
    data object Paused : EngineStatus
    /// The engine stopped answering or does not run on this phone.
    data class Failed(val notStarting: Boolean) : EngineStatus
}

/// What the engine said, and about which position: the screen shows it only while that
/// position is on the board, so an old evaluation never appears for a new one, not
/// even for a frame.
data class EngineView(val position: EnginePosition, val analysis: Analysis)

/// The engine while a game is on the screen: one Stockfish process, the analysis of the
/// shown position only.
class EngineSession(private val files: EngineFiles, private val scope: CoroutineScope) {
    private val _status = MutableStateFlow<EngineStatus>(EngineStatus.Off)
    val status: StateFlow<EngineStatus> = _status.asStateFlow()
    private val _analysis = MutableStateFlow<EngineView?>(null)
    val analysis: StateFlow<EngineView?> = _analysis.asStateFlow()

    private val lock = Mutex()
    @Volatile private var analyzer: Analyzer? = null
    private var loop: Job? = null
    private var timer: Job? = null

    /// The search whose snapshots are taken, and its position (set together).
    private data class Search(val id: ULong, val position: EnginePosition)
    @Volatile private var search: Search? = null
    private var position: EnginePosition? = null
    private var prefs = Prefs()

    /// Bumped by [shutdown]: work queued before it is dropped instead of starting the
    /// engine again.
    @Volatile private var generation = 0

    /// Starts the engine (if it is not running) and analyses `pos`.
    fun show(pos: EnginePosition, p: Prefs) {
        prefs = p
        val changed = pos != position
        position = pos
        val gen = generation
        scope.launch {
            lock.withLock {
                if (gen != generation) return@withLock
                val a = analyzer ?: startLocked(gen) ?: return@withLock
                // The same position again (the screen redrawn, the time over) stays as it is.
                if (changed || _status.value == EngineStatus.Starting) analyzeLocked(a, pos)
            }
        }
    }

    /// Stops thinking (a mate or stalemate on the board); the process stays for the next
    /// position.
    fun pause() {
        position = null
        search = null
        _analysis.value = null
        timer?.cancel()
        val gen = generation
        scope.launch {
            lock.withLock {
                if (gen != generation) return@withLock
                val a = analyzer ?: return@withLock
                withContext(Dispatchers.IO) { a.stop() }
                _status.value = EngineStatus.Paused
            }
        }
    }

    /// Thinks on about the same position after the time ran out.
    fun deeper() {
        val pos = position ?: return
        val gen = generation
        scope.launch {
            lock.withLock {
                if (gen == generation) analyzer?.let { analyzeLocked(it, pos) }
            }
        }
    }

    /// Quits the process (the screen went away, the button was turned off).
    fun shutdown() {
        generation++
        position = null
        search = null
        _analysis.value = null
        _status.value = EngineStatus.Off
        loop?.cancel()
        timer?.cancel()
        // Taken now, so that a show() right after it starts a new engine.
        val a = analyzer ?: return
        analyzer = null
        scope.launch(Dispatchers.IO) {
            lock.withLock {
                a.quit()
                a.close()
            }
        }
    }

    private suspend fun startLocked(gen: Int): Analyzer? {
        _status.value = EngineStatus.Starting
        val a = withContext(Dispatchers.IO) {
            runCatching {
                Analyzer.start(files.executable, files.net.path, prefs.engineThreads.toUInt(), prefs.engineHashMb.toUInt())
            }
        }.getOrElse {
            if (gen == generation) _status.value = EngineStatus.Failed(notStarting = true)
            return null
        }
        if (gen != generation) {
            // Turned off while it was starting.
            withContext(Dispatchers.IO) {
                a.quit()
                a.close()
            }
            return null
        }
        analyzer = a
        loop = scope.launch(Dispatchers.IO) { poll(a) }
        return a
    }

    private suspend fun analyzeLocked(a: Analyzer, pos: EnginePosition) {
        search = null
        val id = withContext(Dispatchers.IO) {
            runCatching { a.analyze(pos.startFen, pos.moves, prefs.engineLines.toUInt()) }
        }.getOrElse {
            _status.value = EngineStatus.Failed(notStarting = false)
            return
        }
        val s = Search(id, pos)
        search = s
        _status.value = EngineStatus.Thinking
        timer?.cancel()
        if (prefs.engineSeconds > 0) {
            timer = scope.launch {
                delay(prefs.engineSeconds * 1000L)
                lock.withLock {
                    if (search == s && analyzer === a) {
                        withContext(Dispatchers.IO) { a.stop() }
                        _status.value = EngineStatus.Paused
                    }
                }
            }
        }
    }

    private suspend fun poll(a: Analyzer) {
        // What the engine said last and was not shown yet.
        var held: EngineView? = null
        var shown = 0L
        while (scope.isActive && analyzer === a) {
            // A short wait: analyze() waits for the engine handle while this holds it.
            val snapshot = try {
                a.poll(40u)
            } catch (e: Exception) {
                if (analyzer === a) _status.value = EngineStatus.Failed(notStarting = false)
                return
            }
            val s = search
            if (snapshot != null && s != null && snapshot.search == s.id && analyzer === a) {
                held = EngineView(s.position, snapshot)
            }
            // The screen follows the engine a few times a second, not at every word of
            // it; the first words about a new position are shown at once.
            val next = held ?: continue
            val now = SystemClock.uptimeMillis()
            if (now - shown >= SHOW_EVERY_MS || next.position != _analysis.value?.position) {
                _analysis.value = next
                held = null
                shown = now
            }
        }
    }

    private companion object {
        const val SHOW_EVERY_MS = 200L
    }
}
