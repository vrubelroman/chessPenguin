package io.github.vrubelroman.chesspenguin

import android.content.Context
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
}
