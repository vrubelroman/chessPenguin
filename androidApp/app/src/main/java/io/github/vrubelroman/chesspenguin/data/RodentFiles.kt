package io.github.vrubelroman.chesspenguin.data

import android.content.Context
import io.github.vrubelroman.chesspenguin.R
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.IOException

/// Rodent IV on the phone (android-rodent.md): the engine is a "library" of the APK
/// (scripts/rodent4.sh), its personalities come with the APK and are copied into its
/// home folder, its opening books are one pack downloaded on request from the authors'
/// repository ([BookPack]). The engine finds both in its home (RODENT4HOME).
class RodentFiles(private val context: Context, scope: CoroutineScope) {
    /// The engine's home: `personalities/` and `books/`.
    val home = File(context.filesDir, "rodent4")

    /// The executable, unpacked by the installer (useLegacyPackaging).
    val executable: String get() = context.applicationInfo.nativeLibraryDir + "/librodent4.so"

    val books = BookPack(context, scope, home)

    /// Copies the personalities out of the APK when they are not there or are of another
    /// version (written beside, then moved in: a cut-off copy is not taken for whole).
    /// False when they could not be (no space): Rodent then does not start.
    suspend fun prepare(): Boolean = withContext(Dispatchers.IO) {
        val version = uniffi.mobile_api.rodentVersion()
        val dir = File(home, "personalities")
        val mark = File(home, "personalities.version")
        if (dir.isDirectory && runCatching { mark.readText() }.getOrNull() == version) return@withContext true
        val fresh = File(home, "personalities.new")
        try {
            fresh.deleteRecursively()
            fresh.mkdirs()
            val assets = context.assets
            for (name in assets.list(ASSETS).orEmpty()) {
                assets.open("$ASSETS/$name").use { input -> File(fresh, name).outputStream().use { input.copyTo(it) } }
            }
            dir.deleteRecursively()
            if (!fresh.renameTo(dir)) throw IOException("cannot move the personalities into place")
            mark.writeText(version)
            true
        } catch (e: IOException) {
            fresh.deleteRecursively()
            false
        }
    }

    private companion object {
        const val ASSETS = "rodent4/personalities"
    }
}

/// Rodent IV's opening books as one pack (android-rodent.md §5): the books its characters
/// open, each a [CheckedDownload] of the system's, fetched one after another and each
/// checked by its SHA-256. A file checked stays: after a cancel or a break only the rest
/// is fetched. The pack is ready only when every file is there.
class BookPack(context: Context, private val scope: CoroutineScope, home: File) {
    private val files: List<CheckedDownload> = uniffi.mobile_api.rodentBookPack().map { b ->
        val local = File(home, b.local)
        CheckedDownload(
            context,
            scope,
            FileSpec(
                dir = local.parentFile!!.relativeTo(context.filesDir).path,
                name = local.name,
                url = b.url,
                size = b.size.toLong(),
                sha256 = b.sha256,
                title = context.getString(R.string.books_notification),
                prefs = "rodent4-book-" + local.name,
                ownFolder = false,
            ),
        )
    }

    /// The pack in all (bytes).
    val size: Long = files.sumOf { it.size }

    private val _state = MutableStateFlow<NetState>(if (installed()) NetState.Ready else NetState.Missing)
    /// Missing (some or all of it), Loading (of what was left when it began), Failed, Ready.
    val state: StateFlow<NetState> = _state.asStateFlow()
    private var job: Job? = null

    private val books = File(home, "books")

    init {
        sweep()
        // A download of the system's still going on from before: the pack goes on.
        if (files.any { it.state.value is NetState.Loading }) download()
    }

    fun installed() = files.all { it.installed() }

    /// Files of another version of the pack (or left half-written) go, and folders left
    /// empty.
    private fun sweep() {
        val known = files.flatMap { listOf(it.file.path, it.file.path + ".part") }.toSet()
        books.walkBottomUp().forEach { f ->
            if (f.isFile && f.path !in known) f.delete()
            if (f.isDirectory && f != books && f.list().isNullOrEmpty()) f.delete()
        }
    }

    /// What is left to download (bytes).
    fun remaining(): Long = files.filterNot { it.installed() }.sumOf { it.size }

    /// Fetches the files not there yet, one after another; stops at the first failure.
    fun download() {
        if (_state.value == NetState.Ready || _state.value is NetState.Loading) return
        val left = files.filterNot { it.installed() }
        val total = left.sumOf { it.size }
        _state.value = NetState.Loading(0, total)
        job = scope.launch(Dispatchers.Main.immediate) {
            var done = 0L
            for (f in left) {
                f.download()
                // Its progress as part of the whole, until it is in place or fails.
                val end = f.state.first { s ->
                    if (s is NetState.Loading) _state.value = NetState.Loading(done + s.done, total)
                    s !is NetState.Loading
                }
                when (end) {
                    NetState.Ready -> done += f.size
                    is NetState.Failed -> {
                        _state.value = end
                        return@launch
                    }
                    // Removed in the system's list of downloads.
                    else -> {
                        _state.value = NetState.Missing
                        return@launch
                    }
                }
            }
            _state.value = if (installed()) NetState.Ready else NetState.Missing
        }
    }

    /// The failure of an earlier attempt is forgotten (the network may be back): the pack
    /// is offered again as it is.
    fun forgetFailure() {
        if (_state.value is NetState.Failed) _state.value = if (installed()) NetState.Ready else NetState.Missing
    }

    /// Stops the download; the files checked so far stay.
    fun cancel() {
        job?.cancel()
        job = null
        files.forEach { if (it.state.value is NetState.Loading) it.cancel() }
        _state.value = if (installed()) NetState.Ready else NetState.Missing
    }

    /// Removes the books (Settings → Engine).
    fun delete() {
        cancel()
        files.forEach { it.delete() }
        sweep()
        _state.value = NetState.Missing
    }
}
