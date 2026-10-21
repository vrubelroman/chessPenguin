package io.github.vrubelroman.chesspenguin.data

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import io.github.vrubelroman.chesspenguin.BuildConfig
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.isActive
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
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
class EngineFiles(private val context: Context, private val scope: CoroutineScope) {
    private val dir = File(context.filesDir, "engine")
    val net = File(dir, BuildConfig.NET_NAME)
    private val part = File(dir, BuildConfig.NET_NAME + ".part")

    /// The executable, unpacked by the installer (useLegacyPackaging).
    val executable: String get() = context.applicationInfo.nativeLibraryDir + "/libstockfish.so"

    private val _state = MutableStateFlow(if (installed()) NetState.Ready else NetState.Missing)
    val state: StateFlow<NetState> = _state.asStateFlow()
    private var job: Job? = null
    @Volatile private var connection: HttpURLConnection? = null

    /// A network checked when it was written: the right size is enough later.
    private fun installed() = net.length() == BuildConfig.NET_SIZE

    val builtIn: Boolean get() = BuildConfig.NET_BUILT_IN

    /// True when the phone is on a metered (mobile) connection right now.
    fun metered(): Boolean {
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return false
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
        return !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
    }

    /// Gets the network ready: from the APK, or from the internet. Runs on in the app's
    /// scope when the screen that asked goes away.
    fun prepare() {
        if (_state.value == NetState.Ready || _state.value is NetState.Loading) return
        // At once: the sheet shows the progress (and "Cancel") from the first moment.
        _state.value = NetState.Loading(part.length().coerceAtMost(BuildConfig.NET_SIZE), BuildConfig.NET_SIZE)
        val previous = job
        job = scope.launch(Dispatchers.IO) {
            // A cancelled download may still be leaving: one writer of the file at a time.
            previous?.cancelAndJoin()
            val result = try {
                dir.mkdirs()
                if (builtIn) copyFromApk() else download()
                NetState.Ready
            } catch (e: NetFailure) {
                NetState.Failed(e.problem)
            } catch (e: IOException) {
                NetState.Failed(if (dir.usableSpace < BuildConfig.NET_SIZE) NetProblem.NoSpace else NetProblem.Interrupted)
            }
            // After "Cancel" the state is already "missing"; the broken connection is no failure.
            if (isActive) _state.value = result
        }
    }

    /// Stops a download; what came is kept for the next one.
    fun cancel() {
        job?.cancel()
        // A read waiting for the network returns at once.
        connection?.disconnect()
        _state.value = NetState.Missing
    }

    /// Removes a downloaded network (Settings → "Delete the engine").
    fun delete() {
        cancel()
        net.delete()
        part.delete()
    }

    private class NetFailure(val problem: NetProblem) : Exception()

    private suspend fun copyFromApk() {
        part.delete()
        context.assets.open(BuildConfig.NET_NAME).use { write(it, append = false, start = 0) }
        finish()
    }

    private suspend fun download() {
        if (!online()) throw NetFailure(NetProblem.NoInternet)
        if (dir.usableSpace + part.length() < BuildConfig.NET_SIZE) throw NetFailure(NetProblem.NoSpace)
        var have = part.length().takeIf { it < BuildConfig.NET_SIZE } ?: 0L
        var url = URL(BuildConfig.NET_URL)
        // GitHub answers with a redirect to its file host; follow it by hand to keep
        // the Range header (resume after an interruption).
        var conn: HttpURLConnection
        var hops = 0
        while (true) {
            conn = url.openConnection() as HttpURLConnection
            connection = conn
            conn.instanceFollowRedirects = false
            conn.connectTimeout = 15_000
            conn.readTimeout = 30_000
            if (have > 0) conn.setRequestProperty("Range", "bytes=$have-")
            val code = try {
                conn.responseCode
            } catch (e: IOException) {
                throw NetFailure(if (online()) NetProblem.Interrupted else NetProblem.NoInternet)
            }
            if (code in 300..399 && hops++ < 5) {
                url = URL(url, conn.getHeaderField("Location"))
                conn.disconnect()
                continue
            }
            if (code == 416) {
                // The part is already whole (or longer): check it.
                conn.disconnect()
                finish()
                return
            }
            if (code != 200 && code != 206) throw NetFailure(NetProblem.Interrupted)
            if (code == 200) have = 0
            break
        }
        try {
            conn.inputStream.use { write(it, append = have > 0, start = have) }
        } catch (e: IOException) {
            throw NetFailure(if (dir.usableSpace < 1_000_000) NetProblem.NoSpace else NetProblem.Interrupted)
        } finally {
            conn.disconnect()
        }
        finish()
    }

    private suspend fun write(input: InputStream, append: Boolean, start: Long) = withContext(Dispatchers.IO) {
        val total = BuildConfig.NET_SIZE
        var done = start
        FileOutputStream(part, append).use { out ->
            val buf = ByteArray(256 * 1024)
            var last = 0L
            while (true) {
                ensureActive()
                val n = input.read(buf)
                if (n < 0) break
                out.write(buf, 0, n)
                done += n
                if (done - last > 512 * 1024 && isActive) {
                    last = done
                    _state.value = NetState.Loading(done, total)
                }
            }
        }
    }

    /// Checks the whole file and puts it in place, or throws it away.
    private fun finish() {
        val digest = MessageDigest.getInstance("SHA-256")
        part.inputStream().use { input ->
            val buf = ByteArray(1024 * 1024)
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                digest.update(buf, 0, n)
            }
        }
        val sum = digest.digest().joinToString("") { "%02x".format(it) }
        if (sum != BuildConfig.NET_SHA256) {
            part.delete()
            throw NetFailure(NetProblem.Corrupt)
        }
        if (!part.renameTo(net)) throw IOException("cannot move the network into place")
    }

    private fun online(): Boolean {
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return true
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
        return caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
    }
}
