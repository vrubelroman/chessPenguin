package io.github.vrubelroman.chesspenguin.data

import android.content.Context
import io.github.vrubelroman.chesspenguin.BuildConfig
import io.github.vrubelroman.chesspenguin.R
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.StateFlow
import java.io.File

/// Stockfish and its network (android-ui.md §8). The executable is a "library" of the
/// APK; the network is a file in the app's storage: copied from the APK in the "full"
/// build, downloaded on request in the "light" one, checked by its SHA-256 either way
/// ([CheckedDownload]).
class EngineFiles(private val context: Context, scope: CoroutineScope) {
    private val download = CheckedDownload(
        context,
        scope,
        FileSpec(
            dir = "engine",
            name = BuildConfig.NET_NAME,
            url = BuildConfig.NET_URL,
            size = BuildConfig.NET_SIZE,
            sha256 = BuildConfig.NET_SHA256,
            title = context.getString(R.string.download_title),
            prefs = "engine",
        ),
    )
    val net: File get() = download.file

    /// The executable, unpacked by the installer (useLegacyPackaging).
    val executable: String get() = context.applicationInfo.nativeLibraryDir + "/libstockfish.so"

    val state: StateFlow<NetState> get() = download.state

    val builtIn: Boolean get() = BuildConfig.NET_BUILT_IN

    /// True when the phone is on a metered (mobile) connection right now.
    fun metered(): Boolean = download.metered()

    /// Gets the network ready: from the APK, or from the internet.
    fun prepare() {
        if (builtIn) download.installFrom { context.assets.open(BuildConfig.NET_NAME) } else download.download()
    }

    /// Keeps the network of the full build in the app's storage, so that it stays after
    /// the app is updated to the light build (android-ui.md §25.6). False when it could
    /// not (no space): the light build then offers the download, as always.
    suspend fun keepBuiltIn(): Boolean = !builtIn || download.installNow { context.assets.open(BuildConfig.NET_NAME) }

    /// Stops a download and throws away what came.
    fun cancel() = download.cancel()

    /// Removes a downloaded network (Settings → "Delete the engine").
    fun delete() = download.delete()
}
