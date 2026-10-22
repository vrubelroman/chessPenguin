package io.github.vrubelroman.chesspenguin

import android.app.Application
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.vrubelroman.chesspenguin.data.EngineFiles
import io.github.vrubelroman.chesspenguin.data.Library
import io.github.vrubelroman.chesspenguin.data.Settings
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import java.util.UUID

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

    /// Changes each time the app goes to the background, and in every new process: the
    /// engine button of a game holds only for the visit it was pressed in (§8.3).
    var visit by mutableStateOf(UUID.randomUUID().toString())
        private set

    fun leftApp() {
        visit = UUID.randomUUID().toString()
    }
}
