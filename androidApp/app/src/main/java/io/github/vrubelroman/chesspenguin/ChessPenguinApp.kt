package io.github.vrubelroman.chesspenguin

import android.app.Application
import io.github.vrubelroman.chesspenguin.data.EngineFiles
import io.github.vrubelroman.chesspenguin.data.Library
import io.github.vrubelroman.chesspenguin.data.Settings
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob

/// The app's databases, settings and engine files live as long as the process.
class ChessPenguinApp : Application() {
    /// Work that outlives a screen (the download of the engine).
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    val library by lazy { Library(this) }
    val settings by lazy { Settings(this) }
    val engineFiles by lazy { EngineFiles(this, scope) }

    /// The games shown on the list of each database (after a search), for paging
    /// games from the game screen: database id → game numbers.
    val shown = HashMap<String, List<UInt>>()
}
