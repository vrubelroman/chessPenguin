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
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
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
    "Pieces cburnett, merida" to "GPLv2+ · Colin M.L. Burnett; Armando Hernandez Marroquin (via Lichess)",
    "Pieces mpchess" to "GPLv3+ · Maxime Chupin (via Lichess)",
    "Pieces chessnut" to "Apache-2.0 · Alexis Luengas (via Lichess)",
    "Pieces fantasy, celtic, spatial" to "MIT · Maurizio Monge (via Lichess)",
    "Pieces rhosgfx" to "CC0-1.0 · RhosGFX (via Lichess)",
    "Pieces shapes" to "CC BY-SA 4.0 · flugsio (via Lichess)",
    "Pieces kiwen-suwi, firi" to "CC BY 4.0 · neverRare; James Faure (via Lichess)",
    "Pieces pixel, letter, pirouetti" to "AGPLv3+ · therealqtpi; usolando; pirouetti (via Lichess)",
    "Pieces governor" to "AGPLv3+ · the lila authors (via PyChess)",
    "Pieces kosal" to "CC BY 4.0 · Kosal Sen, philatype.com (via PyChess)",
    "Pieces metaltops" to "GPL · PyChess",
    "Board images" to "AGPLv3+ · the lila authors and pirouetti (Lichess)",
    "Sounds (SFX)" to "AGPLv3+ · Enigmahack (via Lichess)",
    "Sound of a wrong move" to "CC0-1.0 · Kenney, kenney.nl",
    "Rodent IV 0.33" to "GPL-3.0 · Paweł Koziol, Bernhard C. Maerz (based on Sungorus by Pablo Vázquez) · github.com/nescitus/rodent-iv",
    "Rodent IV opening books (downloaded from its authors on request)" to "Their authors' credits in the books' folders · github.com/nescitus/rodent-iv",
    "Maia-3 (downloaded from its authors on request)" to "CSSLab, University of Toronto · huggingface.co/UofTCSSLab",
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
    val snackbar = remember { SnackbarHostState() }
    UpdateMessages(snackbar)
    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
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
            Text("${BuildConfig.VERSION_NAME} ${stringResource(R.string.alpha)} · $build", color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(24.dp))
            // Right under the version (android-ui.md §25.5).
            AboutUpdates()
            Spacer(Modifier.height(24.dp))
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
