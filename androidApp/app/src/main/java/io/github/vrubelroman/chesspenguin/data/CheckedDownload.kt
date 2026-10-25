package io.github.vrubelroman.chesspenguin.data

import android.app.DownloadManager
import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.storage.StorageManager
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.security.MessageDigest

/// Why a downloaded file is not there.
enum class NetProblem { NoInternet, Interrupted, NoSpace, Corrupt }

sealed interface NetState {
    data object Missing : NetState
    data class Loading(val done: Long, val total: Long) : NetState
    data class Failed(val problem: NetProblem) : NetState
    data object Ready : NetState
}

/// A file of known size and SHA-256 fetched on request into the app's storage: the
/// engine's network, Maia's weights.
data class FileSpec(
    /// Folder in the app's files, and the file's name.
    val dir: String,
    val name: String,
    val url: String,
    val size: Long,
    val sha256: String,
    /// The download's title in the system's notification.
    val title: String,
    /// The preferences that keep the system's download id (one file each).
    val prefs: String,
)

/// The download is the system's (DownloadManager): it goes on with the app closed,
/// waits for the network and resumes, and shows its progress in a notification. Its id
/// is kept, so the app picks the download up again when it starts. The file is checked
/// by its SHA-256 as it is moved into place.
class CheckedDownload(private val context: Context, private val scope: CoroutineScope, private val spec: FileSpec) {
    private val dir = File(context.filesDir, spec.dir)
    val file = File(dir, spec.name)
    private val part = File(dir, spec.name + ".part")
    private val prefs = context.getSharedPreferences(spec.prefs, Context.MODE_PRIVATE)
    private val downloads get() = context.getSystemService(DownloadManager::class.java)

    private val _state = MutableStateFlow<NetState>(NetState.Missing)
    val state: StateFlow<NetState> = _state.asStateFlow()
    private var job: Job? = null

    init {
        // Files of other versions (kept before an update) are of no use to this one.
        dir.listFiles()?.filter { it.name != file.name && it.name != part.name }?.forEach { it.delete() }
        when {
            installed() -> _state.value = NetState.Ready
            downloadId() != NO_DOWNLOAD -> {
                _state.value = NetState.Loading(0, spec.size)
                watch(downloadId())
            }
        }
    }

    /// A file checked when it was written: the right size is enough later.
    fun installed() = file.length() == spec.size

    /// True when the phone is on a metered (mobile) connection right now.
    fun metered(): Boolean {
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return false
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
        return !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
    }

    /// Fetches the file from the internet.
    fun download() {
        if (_state.value == NetState.Ready || _state.value is NetState.Loading) return
        if (!online()) {
            _state.value = NetState.Failed(NetProblem.NoInternet)
            return
        }
        // The file is downloaded next to the app's files, then checked and moved in.
        if (freeSpace() < 2 * spec.size) {
            _state.value = NetState.Failed(NetProblem.NoSpace)
            return
        }
        target()?.delete()
        val request = DownloadManager.Request(Uri.parse(spec.url))
            .setTitle(spec.title)
            .setDestinationInExternalFilesDir(context, null, spec.name)
            .setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE)
            .setAllowedOverMetered(true)
        val id = try {
            downloads.enqueue(request)
        } catch (e: IllegalArgumentException) {
            // No shared storage for the download (or the download service is off).
            _state.value = NetState.Failed(NetProblem.Interrupted)
            return
        }
        prefs.edit().putLong(KEY, id).apply()
        _state.value = NetState.Loading(0, spec.size)
        watch(id)
    }

    /// Writes the file from `open()` (inside the APK) into place, checked, in the
    /// background, the progress shown as a download's.
    fun installFrom(open: () -> InputStream) {
        if (_state.value == NetState.Ready || _state.value is NetState.Loading) return
        _state.value = NetState.Loading(0, spec.size)
        job = scope.launch(Dispatchers.IO) {
            val result = try {
                open().use { install(it, progress = true) }
                NetState.Ready
            } catch (e: Failure) {
                NetState.Failed(e.problem)
            } catch (e: IOException) {
                NetState.Failed(if (freeSpace() < spec.size) NetProblem.NoSpace else NetProblem.Interrupted)
            }
            if (isActive) _state.value = result
        }
    }

    /// The same, at once (the caller waits): false when it could not (no space).
    suspend fun installNow(open: () -> InputStream): Boolean = withContext(Dispatchers.IO) {
        if (installed()) return@withContext true
        if (freeSpace() < 2 * spec.size) return@withContext false
        try {
            open().use { install(it, progress = false) }
            _state.value = NetState.Ready
            true
        } catch (e: Exception) {
            part.delete()
            if (!installed()) _state.value = NetState.Missing
            false
        }
    }

    /// Stops a download and throws away what came.
    fun cancel() {
        job?.cancel()
        val id = downloadId()
        if (id != NO_DOWNLOAD) downloads.remove(id)
        forget()
        part.delete()
        if (!installed()) _state.value = NetState.Missing
    }

    /// Removes the file.
    fun delete() {
        cancel()
        file.delete()
        _state.value = NetState.Missing
    }

    private class Failure(val problem: NetProblem) : Exception()

    private fun downloadId(): Long = prefs.getLong(KEY, NO_DOWNLOAD)

    private fun forget() = prefs.edit().remove(KEY).apply()

    /// Where the system puts the downloaded file (the app's own external folder).
    private fun target(): File? = context.getExternalFilesDir(null)?.let { File(it, spec.name) }

    /// Follows a download of the system until it ends. The state is set on the main
    /// thread, as "Cancel" sets it: after a cancel the watcher sets nothing more.
    private fun watch(id: Long) {
        job = scope.launch(Dispatchers.Main.immediate) {
            while (isActive) {
                val row = withContext(Dispatchers.IO) { progress(id) }
                if (row == null) {
                    // Removed in the system's list of downloads.
                    forget()
                    _state.value = NetState.Missing
                    return@launch
                }
                when (row.status) {
                    DownloadManager.STATUS_SUCCESSFUL -> {
                        val result = withContext(Dispatchers.IO) {
                            try {
                                val f = target() ?: throw IOException("no external folder")
                                f.inputStream().use { install(it, progress = false) }
                                NetState.Ready
                            } catch (e: Failure) {
                                NetState.Failed(e.problem)
                            } catch (e: IOException) {
                                NetState.Failed(if (freeSpace() < spec.size) NetProblem.NoSpace else NetProblem.Interrupted)
                            }
                        }
                        downloads.remove(id)
                        forget()
                        _state.value = result
                        return@launch
                    }
                    DownloadManager.STATUS_FAILED -> {
                        downloads.remove(id)
                        forget()
                        _state.value = NetState.Failed(
                            if (row.reason == DownloadManager.ERROR_INSUFFICIENT_SPACE) NetProblem.NoSpace else NetProblem.Interrupted,
                        )
                        return@launch
                    }
                    else -> _state.value = NetState.Loading(row.done.coerceAtLeast(0), spec.size)
                }
                delay(500)
            }
        }
    }

    private class Row(val status: Int, val reason: Int, val done: Long)

    private fun progress(id: Long): Row? =
        downloads.query(DownloadManager.Query().setFilterById(id))?.use { c ->
            if (!c.moveToFirst()) return null
            Row(
                c.getInt(c.getColumnIndexOrThrow(DownloadManager.COLUMN_STATUS)),
                c.getInt(c.getColumnIndexOrThrow(DownloadManager.COLUMN_REASON)),
                c.getLong(c.getColumnIndexOrThrow(DownloadManager.COLUMN_BYTES_DOWNLOADED_SO_FAR)),
            )
        }

    /// Writes the file into place, checking its SHA-256 on the way; a wrong file is
    /// thrown away.
    private suspend fun install(input: InputStream, progress: Boolean) {
        dir.mkdirs()
        val digest = MessageDigest.getInstance("SHA-256")
        var done = 0L
        part.outputStream().use { out ->
            val buf = ByteArray(256 * 1024)
            var last = 0L
            while (true) {
                kotlin.coroutines.coroutineContext.ensureActive()
                val n = input.read(buf)
                if (n < 0) break
                out.write(buf, 0, n)
                digest.update(buf, 0, n)
                done += n
                if (progress && done - last > 512 * 1024) {
                    last = done
                    _state.value = NetState.Loading(done, spec.size)
                }
            }
        }
        val sum = digest.digest().joinToString("") { "%02x".format(it) }
        if (sum != spec.sha256 || done != spec.size) {
            part.delete()
            throw Failure(NetProblem.Corrupt)
        }
        if (!part.renameTo(file)) throw IOException("cannot move the file into place")
    }

    /// The space the app can take, with the caches the system would clear for it. Asked
    /// of filesDir, which always exists: a folder not made yet answers 0.
    private fun freeSpace(): Long = try {
        val storage = context.getSystemService(StorageManager::class.java)
        storage.getAllocatableBytes(storage.getUuidForPath(context.filesDir))
    } catch (e: IOException) {
        context.filesDir.usableSpace
    }

    private fun online(): Boolean {
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return true
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
        return caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
    }

    private companion object {
        const val KEY = "download"
        const val NO_DOWNLOAD = -1L
    }
}
