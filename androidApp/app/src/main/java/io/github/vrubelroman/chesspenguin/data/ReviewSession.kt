package io.github.vrubelroman.chesspenguin.data

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import uniffi.mobile_api.Database
import uniffi.mobile_api.GameReview
import uniffi.mobile_api.ReviewState

/// Nodes per position of the three qualities, as on the desktop (android-ui.md §21.2).
val REVIEW_NODES = listOf(250_000UL, 1_000_000UL, 3_000_000UL)

/// A first guess at the engine's speed per thread before it was measured on this phone.
const val GUESS_NPS_PER_THREAD = 250_000L

/// The analysis of the game on the screen (its report, android-ui.md §21): the cached
/// or PGN evaluations at once, the engine job when asked for. One game at a time; the
/// job has its own engine process (the Rust side runs it on a thread of its own).
class ReviewSession(private val cacheDir: String, private val scope: CoroutineScope) {
    private val _state = MutableStateFlow<ReviewState?>(null)
    /// The analysis of the game shown, `null` while it loads.
    val state: StateFlow<ReviewState?> = _state.asStateFlow()

    private val lock = Mutex()
    private var review: GameReview? = null
    private var game: Pair<String, UInt>? = null
    private var poller: Job? = null

    /// Bumped whenever another game comes: work for the old one is dropped.
    @Volatile private var generation = 0

    /// Shows the analysis of game `index` of database `db` (from the cache or its
    /// `[%eval]` comments); the analysis of the game before is stopped and kept.
    suspend fun open(db: String, database: Database, index: UInt) = openWith(db to index) { database.review(index, cacheDir) }

    /// The analysis of a game being changed or played (no file of its own), as it is
    /// now; `stamp` names this state of it (a new stamp: the game changed).
    suspend fun openSession(stamp: String, session: uniffi.mobile_api.GameSession) = openWith("session" to stamp.hashCode().toUInt()) { session.review(cacheDir) }

    private suspend fun openWith(key: Pair<String, UInt>, make: () -> GameReview) {
        if (game == key) return
        val gen = ++generation
        _state.value = null
        lock.withLock {
            closeLocked()
            val r = withContext(Dispatchers.IO) { runCatching { make() }.getOrNull() } ?: return
            if (gen != generation) {
                withContext(Dispatchers.IO) { r.close() }
                return
            }
            review = r
            game = key
            _state.value = withContext(Dispatchers.IO) { r.state() }
        }
    }

    /// Starts the engine on the positions without an evaluation, or on all when `full`.
    /// `done` gets the final state when the job ends by itself.
    fun start(path: String, evalFile: String, threads: Int, hashMb: Int, nodes: ULong, full: Boolean, done: (ReviewState) -> Unit) {
        val gen = generation
        scope.launch {
            lock.withLock {
                val r = review ?: return@withLock
                if (gen != generation) return@withLock
                val started = withContext(Dispatchers.IO) {
                    r.start(path, evalFile, threads.toUInt(), hashMb.toUInt(), nodes, full)
                }
                _state.value = withContext(Dispatchers.IO) { r.state() }
                if (started) poll(r, gen, done)
            }
        }
    }

    /// Stops the engine; what it evaluated stays (and is cached).
    fun cancel() {
        poller?.cancel()
        val gen = generation
        scope.launch {
            lock.withLock {
                val r = review ?: return@withLock
                withContext(Dispatchers.IO) { r.cancel() }
                if (gen == generation) _state.value = withContext(Dispatchers.IO) { r.state() }
            }
        }
    }

    /// The screen went away: the job stops and the object is freed.
    fun close() {
        generation++
        poller?.cancel()
        scope.launch { lock.withLock { closeLocked() } }
    }

    private suspend fun closeLocked() {
        poller?.cancel()
        val r = review ?: return
        review = null
        game = null
        withContext(Dispatchers.IO) {
            r.cancel()
            r.close()
        }
    }

    /// While the job runs: its state a few times a second (the graph grows with it).
    private fun poll(r: GameReview, gen: Int, done: (ReviewState) -> Unit) {
        poller?.cancel()
        poller = scope.launch(Dispatchers.IO) {
            while (true) {
                delay(POLL_MS)
                val s = lock.withLock {
                    if (review !== r || gen != generation) null else r.state()
                } ?: return@launch
                _state.value = s
                if (!s.running) {
                    done(s)
                    return@launch
                }
            }
        }
    }

    private companion object {
        const val POLL_MS = 300L
    }
}
