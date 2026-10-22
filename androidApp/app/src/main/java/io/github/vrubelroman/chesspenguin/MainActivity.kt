package io.github.vrubelroman.chesspenguin

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.ui.Modifier
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import io.github.vrubelroman.chesspenguin.data.Language
import io.github.vrubelroman.chesspenguin.data.Theme
import io.github.vrubelroman.chesspenguin.theme.ChessPenguinTheme

class MainActivity : ComponentActivity() {
    override fun attachBaseContext(base: Context) {
        super.attachBaseContext(Language.wrap(base))
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        // Not again when the activity is made anew (turned): it was taken the first time.
        if (savedInstanceState == null) receive(intent)
        val settings = (application as ChessPenguinApp).settings
        setContent {
            val prefs by settings.prefs.collectAsState()
            val dark = when (prefs.theme) {
                Theme.System -> isSystemInDarkTheme()
                Theme.Light -> false
                Theme.Dark -> true
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

    override fun onStop() {
        super.onStop()
        // Gone to the background or closed, not turned.
        if (!isChangingConfigurations) (application as ChessPenguinApp).leftApp()
    }
}
