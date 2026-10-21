package io.github.vrubelroman.chesspenguin.data

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

/// The engine while a game is on the screen: one Stockfish process, the analysis of the
/// shown position only. A new position clears the lines at once; snapshots of an older
/// search are dropped, so an old evaluation is never shown for a new position.
class EngineSession(private val files: EngineFiles, private val scope: CoroutineScope) {
    private val _status = MutableStateFlow<EngineStatus>(EngineStatus.Off)
    val status: StateFlow<EngineStatus> = _status.asStateFlow()
    private val _analysis = MutableStateFlow<Analysis?>(null)
    val analysis: StateFlow<Analysis?> = _analysis.asStateFlow()

    private val lock = Mutex()
    private var analyzer: Analyzer? = null
    private var loop: Job? = null
    private var timer: Job? = null
    @Volatile private var search: ULong = 0u
    private var position: EnginePosition? = null
    private var prefs = Prefs()

    /// Starts the engine (if it is not running) and analyses `pos`.
    fun show(pos: EnginePosition, p: Prefs) {
        prefs = p
        val changed = pos != position
        position = pos
        scope.launch {
            lock.withLock {
                val a = analyzer ?: startLocked() ?: return@withLock
                // The same position again (the screen redrawn, the time over) stays as it is.
                if (changed || _status.value == EngineStatus.Starting) analyzeLocked(a, pos)
            }
        }
    }

    /// Thinks on about the same position after the time ran out.
    fun deeper() {
        val pos = position ?: return
        scope.launch { lock.withLock { analyzer?.let { analyzeLocked(it, pos) } } }
    }

    /// Quits the process (the screen went away, the button was turned off).
    fun shutdown() {
        position = null
        _analysis.value = null
        _status.value = EngineStatus.Off
        val a = analyzer
        analyzer = null
        loop?.cancel()
        timer?.cancel()
        if (a != null) scope.launch(Dispatchers.IO) { lock.withLock { a.quit(); a.close() } }
    }

    private suspend fun startLocked(): Analyzer? {
        _status.value = EngineStatus.Starting
        val a = withContext(Dispatchers.IO) {
            runCatching {
                Analyzer.start(files.executable, files.net.path, prefs.engineThreads.toUInt(), prefs.engineHashMb.toUInt())
            }
        }.getOrElse {
            _status.value = EngineStatus.Failed(notStarting = true)
            return null
        }
        analyzer = a
        loop = scope.launch(Dispatchers.IO) { poll(a) }
        return a
    }

    private suspend fun analyzeLocked(a: Analyzer, pos: EnginePosition) {
        // No snapshot is taken until the new search has its number.
        search = 0u
        _analysis.value = null
        val id = withContext(Dispatchers.IO) {
            runCatching { a.analyze(pos.startFen, pos.moves, prefs.engineLines.toUInt()) }
        }.getOrElse {
            _status.value = EngineStatus.Failed(notStarting = false)
            return
        }
        search = id
        _status.value = EngineStatus.Thinking
        timer?.cancel()
        if (prefs.engineSeconds > 0) {
            timer = scope.launch {
                delay(prefs.engineSeconds * 1000L)
                lock.withLock {
                    if (search == id && analyzer === a) {
                        withContext(Dispatchers.IO) { a.stop() }
                        _status.value = EngineStatus.Paused
                    }
                }
            }
        }
    }

    private suspend fun poll(a: Analyzer) {
        while (scope.isActive && analyzer === a) {
            val snapshot = try {
                a.poll(150u)
            } catch (e: Exception) {
                if (analyzer === a) _status.value = EngineStatus.Failed(notStarting = false)
                return
            }
            if (snapshot != null && snapshot.search == search && analyzer === a) _analysis.value = snapshot
        }
    }
}
