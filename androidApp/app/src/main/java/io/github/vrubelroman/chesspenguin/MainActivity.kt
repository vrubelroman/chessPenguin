package io.github.vrubelroman.chesspenguin

import android.content.Context
import android.app.UiModeManager
import android.content.Intent
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.launch
import android.net.Uri
import android.os.Bundle
import android.graphics.Color
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.ui.Modifier
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import io.github.vrubelroman.chesspenguin.data.Language
import io.github.vrubelroman.chesspenguin.data.Theme
import io.github.vrubelroman.chesspenguin.theme.ChessPenguinTheme

/// The scrims behind three-button navigation, as `enableEdgeToEdge` has them by default.
private val LIGHT_SCRIM = Color.argb(0xe6, 0xFF, 0xFF, 0xFF)
private val DARK_SCRIM = Color.argb(0x80, 0x1b, 0x1b, 0x1b)

class MainActivity : ComponentActivity() {
    override fun attachBaseContext(base: Context) {
        super.attachBaseContext(Language.wrap(base))
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        // Not again when the activity is made anew (turned): it was taken the first time.
        if (savedInstanceState == null) receive(intent)
        val app = application as ChessPenguinApp
        val settings = app.settings
        // Android's question «Update this app?» is put on the screen only while the app is
        // on it (the installer may answer while the user is elsewhere).
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.RESUMED) {
                app.updates.confirm.collect { confirm ->
                    if (confirm != null) {
                        app.updates.confirmShown()
                        startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                    }
                }
            }
        }
        setContent {
            val prefs by settings.prefs.collectAsState()
            val dark = when (prefs.theme) {
                Theme.System -> isSystemInDarkTheme()
                Theme.Light -> false
                Theme.Dark -> true
            }
            // Android keeps the app's own day or night (Android 12+): the window and the
            // splash screen before Compose draws then match the theme chosen, not the
            // system's (no white flash before a dark app).
            LaunchedEffect(prefs.theme) {
                getSystemService(UiModeManager::class.java)?.setApplicationNightMode(
                    when (prefs.theme) {
                        Theme.System -> UiModeManager.MODE_NIGHT_AUTO
                        Theme.Light -> UiModeManager.MODE_NIGHT_NO
                        Theme.Dark -> UiModeManager.MODE_NIGHT_YES
                    },
                )
            }
            // The system bars follow the theme chosen in the app, not the system's: light
            // icons over a dark app even when the phone itself is light, and back.
            DisposableEffect(dark) {
                enableEdgeToEdge(
                    statusBarStyle = SystemBarStyle.auto(Color.TRANSPARENT, Color.TRANSPARENT) { dark },
                    navigationBarStyle = SystemBarStyle.auto(LIGHT_SCRIM, DARK_SCRIM) { dark },
                )
                onDispose {}
            }
            ChessPenguinTheme(darkTheme = dark) {
                Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) { MainNavigation() }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        receive(intent)
    }

    /// A file given by another app: "Open with" (VIEW) or "Share" (SEND).
    private fun receive(intent: Intent?) {
        // Opened again from Recents: the task keeps the intent of the file it was made for.
        if (intent == null || intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0) return
        val uri = when (intent.action) {
            Intent.ACTION_VIEW -> intent.data
            Intent.ACTION_SEND -> androidx.core.content.IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)
            else -> null
        } ?: return
        (application as ChessPenguinApp).receive(uri)
    }

    override fun onStart() {
        super.onStart()
        val updates = (application as ChessPenguinApp).updates
        updates.foreground = true
        updates.checkIfDue()
        // The downloaded databases get their new games (at most every ten minutes).
        (application as ChessPenguinApp).downloads.updateAllQuietly()
    }

    override fun onResume() {
        super.onResume()
        (application as ChessPenguinApp).updates.onResume()
    }

    override fun onStop() {
        super.onStop()
        (application as ChessPenguinApp).updates.foreground = false
        // Gone to the background or closed, not turned.
        if (!isChangingConfigurations) (application as ChessPenguinApp).leftApp()
    }
}
