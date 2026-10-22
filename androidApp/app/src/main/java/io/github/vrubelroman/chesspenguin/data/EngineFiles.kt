package io.github.vrubelroman.chesspenguin.data

import android.app.DownloadManager
import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.storage.StorageManager
import io.github.vrubelroman.chesspenguin.BuildConfig
import io.github.vrubelroman.chesspenguin.R
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

/// Why the network of the engine is not there.
enum class NetProblem { NoInternet, Interrupted, NoSpace, Corrupt }

sealed interface NetState {
    data object Missing : NetState
    data class Loading(val done: Long, val total: Long) : NetState
    data class Failed(val problem: NetProblem) : NetState
    data object Ready : NetState
}

/// Stockfish and its network (android-ui.md §8). The executable is a "library" of the
/// APK; the network is a file in the app's storage: copied from the APK in the "full"
/// build, downloaded on request in the "light" one, checked by its SHA-256 either way.
///
/// The download is the system's (DownloadManager): it goes on with the app closed,
/// waits for the network and resumes, and shows its progress in a notification. Its
/// id is kept, so the app picks the download up again when it starts.
class EngineFiles(private val context: Context, private val scope: CoroutineScope) {
    private val dir = File(context.filesDir, "engine")
    val net = File(dir, BuildConfig.NET_NAME)
    private val part = File(dir, BuildConfig.NET_NAME + ".part")
    private val prefs = context.getSharedPreferences("engine", Context.MODE_PRIVATE)
    private val downloads get() = context.getSystemService(DownloadManager::class.java)

    /// The executable, unpacked by the installer (useLegacyPackaging).
    val executable: String get() = context.applicationInfo.nativeLibraryDir + "/libstockfish.so"

    private val _state = MutableStateFlow<NetState>(NetState.Missing)
    val state: StateFlow<NetState> = _state.asStateFlow()
    private var job: Job? = null

    init {
        when {
            installed() -> _state.value = NetState.Ready
            download() != NO_DOWNLOAD -> {
                _state.value = NetState.Loading(0, BuildConfig.NET_SIZE)
                watch(download())
            }
        }
    }

    /// A network checked when it was written: the right size is enough later.
    private fun installed() = net.length() == BuildConfig.NET_SIZE

    val builtIn: Boolean get() = BuildConfig.NET_BUILT_IN

    /// True when the phone is on a metered (mobile) connection right now.
    fun metered(): Boolean {
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return false
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
        return !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
    }

    /// Gets the network ready: from the APK, or from the internet.
    fun prepare() {
        if (_state.value == NetState.Ready || _state.value is NetState.Loading) return
        if (builtIn) {
            _state.value = NetState.Loading(0, BuildConfig.NET_SIZE)
            job = scope.launch(Dispatchers.IO) {
                val result = try {
                    dir.mkdirs()
                    context.assets.open(BuildConfig.NET_NAME).use { install(it) }
                    NetState.Ready
                } catch (e: NetFailure) {
                    NetState.Failed(e.problem)
                } catch (e: IOException) {
                    NetState.Failed(if (freeSpace() < BuildConfig.NET_SIZE) NetProblem.NoSpace else NetProblem.Interrupted)
                }
                if (isActive) _state.value = result
            }
            return
        }
        if (!online()) {
            _state.value = NetState.Failed(NetProblem.NoInternet)
            return
        }
        // The file is downloaded next to the app's files, then checked and moved in.
        if (freeSpace() < 2 * BuildConfig.NET_SIZE) {
            _state.value = NetState.Failed(NetProblem.NoSpace)
            return
        }
        target()?.delete()
        val request = DownloadManager.Request(Uri.parse(BuildConfig.NET_URL))
            .setTitle(context.getString(R.string.download_title))
            .setDestinationInExternalFilesDir(context, null, BuildConfig.NET_NAME)
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
        _state.value = NetState.Loading(0, BuildConfig.NET_SIZE)
        watch(id)
    }

    /// Stops a download and throws away what came.
    fun cancel() {
        job?.cancel()
        val id = download()
        if (id != NO_DOWNLOAD) downloads.remove(id)
        forget()
        part.delete()
        if (!installed()) _state.value = NetState.Missing
    }

    /// Removes a downloaded network (Settings → "Delete the engine").
    fun delete() {
        cancel()
        net.delete()
        _state.value = NetState.Missing
    }

    private class NetFailure(val problem: NetProblem) : Exception()

    private fun download(): Long = prefs.getLong(KEY, NO_DOWNLOAD)

    private fun forget() = prefs.edit().remove(KEY).apply()

    /// Where the system puts the downloaded file (the app's own external folder).
    private fun target(): File? = context.getExternalFilesDir(null)?.let { File(it, BuildConfig.NET_NAME) }

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
                                dir.mkdirs()
                                val file = target() ?: throw IOException("no external folder")
                                file.inputStream().use { install(it) }
                                NetState.Ready
                            } catch (e: NetFailure) {
                                NetState.Failed(e.problem)
                            } catch (e: IOException) {
                                NetState.Failed(if (freeSpace() < BuildConfig.NET_SIZE) NetProblem.NoSpace else NetProblem.Interrupted)
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
                    else -> _state.value = NetState.Loading(row.done.coerceAtLeast(0), BuildConfig.NET_SIZE)
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

    /// Writes the network into place, checking its SHA-256 on the way; a wrong file is
    /// thrown away.
    private suspend fun install(input: InputStream) {
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
                if (builtIn && done - last > 512 * 1024) {
                    last = done
                    _state.value = NetState.Loading(done, BuildConfig.NET_SIZE)
                }
            }
        }
        val sum = digest.digest().joinToString("") { "%02x".format(it) }
        if (sum != BuildConfig.NET_SHA256 || done != BuildConfig.NET_SIZE) {
            part.delete()
            throw NetFailure(NetProblem.Corrupt)
        }
        if (!part.renameTo(net)) throw IOException("cannot move the network into place")
    }

    /// The space the app can take, with the caches the system would clear for it. Asked
    /// of filesDir, which always exists: a folder not made yet (the engine's, before the
    /// first download) answers 0.
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
