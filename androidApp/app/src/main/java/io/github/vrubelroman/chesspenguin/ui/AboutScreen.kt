package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import io.github.vrubelroman.chesspenguin.BuildConfig
import io.github.vrubelroman.chesspenguin.R

private const val SITE = "https://chess.vrubel.xyz"

/// The authors and licenses of what the app is made of (android-ui.md §8.3, §9).
private val NOTICES = listOf(
    "ChessPenguin" to "GPL-3.0-or-later · vrubelroman",
    "Stockfish 19" to "GPL-3.0 · The Stockfish developers (see AUTHORS) · github.com/official-stockfish/Stockfish",
    "Merida pieces" to "GPLv2+ · Armando Hernandez Marroquin (via Lichess)",
    "Figurines, Noto Sans Math" to "SIL Open Font License 1.1",
    "Jetpack Compose, AndroidX, Kotlin" to "Apache-2.0 · Google, JetBrains",
    "UniFFI" to "MPL-2.0 · Mozilla",
    "JNA" to "Apache-2.0 / LGPL-2.1",
    "AndroidSVG" to "Apache-2.0 · Paul LeBeau",
    "shakmaty" to "GPL-3.0-or-later · Niklas Fiekas",
)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AboutScreen(onBack: () -> Unit) {
    val uri = LocalUriHandler.current
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.about)) },
                navigationIcon = {
                    IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Rounded.ArrowBack, stringResource(R.string.back)) }
                },
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp)) {
            Image(painterResource(R.drawable.penguin), null, Modifier.size(72.dp))
            Spacer(Modifier.height(16.dp))
            Text(stringResource(R.string.app_name), style = MaterialTheme.typography.titleLarge)
            val build = stringResource(if (BuildConfig.NET_BUILT_IN) R.string.build_full else R.string.build_light)
            Text("${BuildConfig.VERSION_NAME} · $build", color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(16.dp))
            Text(stringResource(R.string.about_text), style = MaterialTheme.typography.bodyLarge)
            TextButton(onClick = { uri.openUri(SITE) }, contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp)) {
                Text(stringResource(R.string.website))
            }
            Spacer(Modifier.height(16.dp))
            Text(stringResource(R.string.licenses), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
            NOTICES.forEach { (what, license) ->
                Spacer(Modifier.height(12.dp))
                Text(what, style = MaterialTheme.typography.bodyLarge)
                Text(license, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}
