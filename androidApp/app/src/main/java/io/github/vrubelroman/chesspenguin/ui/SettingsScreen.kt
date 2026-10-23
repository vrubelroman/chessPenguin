package io.github.vrubelroman.chesspenguin.ui

import android.app.Activity
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.takeOrElse
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import java.util.Locale
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import io.github.vrubelroman.chesspenguin.BuildConfig
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.LANGUAGES
import io.github.vrubelroman.chesspenguin.data.Language
import io.github.vrubelroman.chesspenguin.data.NetState
import io.github.vrubelroman.chesspenguin.data.Notation
import io.github.vrubelroman.chesspenguin.data.Theme

/// One screen of groups, changes apply at once (android-ui.md §9). The board and the
/// pieces are chosen on a screen of their own, with a board to see them on.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(onBack: () -> Unit, onAbout: () -> Unit, onBoardStyle: () -> Unit) {
    val app = app()
    val prefs by app.settings.prefs.collectAsState()
    val files = app.engineFiles
    val net by files.state.collectAsState()
    val set = app.settings::update
    var choice by remember { mutableStateOf<Choice?>(null) }
    var deleting by remember { mutableStateOf(false) }
    var sheet by remember { mutableStateOf(false) }
    val lang = uiLanguage()
    val scroll = TopAppBarDefaults.pinnedScrollBehavior()
    val snackbar = remember { androidx.compose.material3.SnackbarHostState() }
    UpdateMessages(snackbar)
    val size = (BuildConfig.NET_SIZE / 1_000_000).toInt()

    val themeNames = listOf(
        Theme.System to stringResource(R.string.theme_system),
        Theme.Light to stringResource(R.string.theme_light),
        Theme.Dark to stringResource(R.string.theme_dark),
    )
    val notationNames = buildList {
        add(Notation.Latin to stringResource(R.string.notation_latin))
        add(Notation.Figurines to stringResource(R.string.notation_figurines))
        // English has no letters of its own (move-notation.md §2.1).
        if (lang != "en") add(Notation.Local to stringResource(R.string.notation_local))
    }
    val secondsNames = listOf(
        10 to stringResource(R.string.seconds, 10),
        20 to stringResource(R.string.seconds, 20),
        60 to stringResource(R.string.one_minute),
        0 to stringResource(R.string.no_limit),
    )
    val cores = Runtime.getRuntime().availableProcessors()

    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection),
        snackbarHost = { androidx.compose.material3.SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.settings)) },
                navigationIcon = {
                    IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Rounded.ArrowBack, stringResource(R.string.back)) }
                },
                scrollBehavior = scroll,
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
            Group(stringResource(R.string.group_game))
            val notationNow = notationNames.firstOrNull { it.first == prefs.notation } ?: notationNames.first()
            Item(stringResource(R.string.notation), notationNow.second) {
                choice = Choice(
                    R.string.notation,
                    notationNames.map { it.second },
                    notationNames.indexOf(notationNow),
                    hint = R.string.notation_hint,
                ) { i -> set { it.copy(notation = notationNames[i].first) } }
            }

            Toggle(stringResource(R.string.animation), prefs.animation) { v -> set { it.copy(animation = v) } }
            Toggle(stringResource(R.string.best_arrow), prefs.bestArrow) { v -> set { it.copy(bestArrow = v) } }
            Toggle(stringResource(R.string.vibration), prefs.vibration) { v -> set { it.copy(vibration = v) } }

            Group(stringResource(R.string.group_look))
            Item(stringResource(R.string.board_and_pieces), boardLabel(prefs.board) + " · " + prefs.pieces.dir, onClick = onBoardStyle)
            Item(stringResource(R.string.theme), themeNames.first { it.first == prefs.theme }.second) {
                choice = Choice(R.string.theme, themeNames.map { it.second }, themeNames.indexOfFirst { it.first == prefs.theme }) { i ->
                    set { it.copy(theme = themeNames[i].first) }
                }
            }
            val context = LocalContext.current
            val languageNames = LANGUAGES.map {
                if (it.isEmpty()) stringResource(R.string.theme_system) else Locale.forLanguageTag(it).let { l -> l.getDisplayLanguage(l).replaceFirstChar(Char::uppercase) }
            }
            val language = remember { Language.get(context) }
            Item(stringResource(R.string.language), languageNames[LANGUAGES.indexOf(language).coerceAtLeast(0)]) {
                choice = Choice(R.string.language, languageNames, LANGUAGES.indexOf(language).coerceAtLeast(0)) { i ->
                    if (Language.set(context, LANGUAGES[i])) (context as? Activity)?.recreate()
                }
            }

            Group(stringResource(R.string.group_engine))
            when {
                files.builtIn -> Status(stringResource(R.string.engine_built_in))
                net == NetState.Ready -> {
                    Status(stringResource(R.string.engine_downloaded))
                    Item(stringResource(R.string.engine_delete, size), null) { deleting = true }
                }
                net is NetState.Loading -> {
                    val l = net as NetState.Loading
                    Item(stringResource(R.string.engine_downloading, (l.done / 1_000_000).toInt(), (l.total / 1_000_000).toInt()), null) { sheet = true }
                }
                // The same sheet as in a game: the size, mobile data, cancel (§9).
                else -> Item(stringResource(R.string.engine_missing, size), null) { sheet = true }
            }
            Item(stringResource(R.string.lines), "${prefs.engineLines}") {
                choice = Choice(R.string.lines, listOf("1", "2", "3"), prefs.engineLines - 1) { i -> set { it.copy(engineLines = i + 1) } }
            }
            Item(stringResource(R.string.time_per_position), secondsNames.firstOrNull { it.first == prefs.engineSeconds }?.second ?: "") {
                choice = Choice(R.string.time_per_position, secondsNames.map { it.second }, secondsNames.indexOfFirst { it.first == prefs.engineSeconds }) { i ->
                    set { it.copy(engineSeconds = secondsNames[i].first) }
                }
            }
            Item(stringResource(R.string.threads), "${prefs.engineThreads}") {
                choice = Choice(R.string.threads, (1..cores).map { "$it" }, prefs.engineThreads - 1) { i -> set { it.copy(engineThreads = i + 1) } }
            }
            val hashes = listOf(16, 32, 64, 128, 256)
            val hashNames = hashes.map { stringResource(R.string.megabytes, it) }
            Item(stringResource(R.string.memory), stringResource(R.string.megabytes, prefs.engineHashMb)) {
                choice = Choice(R.string.memory, hashNames, hashes.indexOf(prefs.engineHashMb)) { i -> set { it.copy(engineHashMb = hashes[i]) } }
            }
            Text(
                stringResource(R.string.engine_battery),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
            )
            Spacer(Modifier.height(16.dp))
            // A newer version of the app, even with its card hidden (android-ui.md §25.5).
            Item(stringResource(R.string.about), aboutUpdateLine(), valueColor = MaterialTheme.colorScheme.primary, onClick = onAbout)
        }
    }

    choice?.let { c ->
        AlertDialog(
            onDismissRequest = { choice = null },
            title = { Text(stringResource(c.title)) },
            text = {
                Column {
                    c.options.forEachIndexed { i, label ->
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .heightIn(min = 48.dp)
                                .selectable(selected = i == c.selected, role = Role.RadioButton) {
                                    c.pick(i)
                                    choice = null
                                },
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            RadioButton(selected = i == c.selected, onClick = null)
                            Text(label, Modifier.padding(start = 16.dp), style = MaterialTheme.typography.bodyLarge)
                        }
                    }
                    c.hint?.let {
                        Spacer(Modifier.height(8.dp))
                        Text(stringResource(it), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            },
            confirmButton = { TextButton(onClick = { choice = null }) { Text(stringResource(R.string.cancel)) } },
        )
    }
    if (sheet) {
        EngineSheet(
            state = net,
            metered = remember(sheet) { files.metered() },
            onDownload = { files.prepare() },
            onCancel = {
                files.cancel()
                sheet = false
            },
            onDismiss = { sheet = false },
        )
    }
    LaunchedEffect(net) { if (net == NetState.Ready) sheet = false }
    if (deleting) {
        AlertDialog(
            onDismissRequest = { deleting = false },
            text = { Text(stringResource(R.string.engine_delete_question)) },
            confirmButton = {
                TextButton(onClick = {
                    files.delete()
                    deleting = false
                }) { Text(stringResource(R.string.delete)) }
            },
            dismissButton = { TextButton(onClick = { deleting = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}

private class Choice(val title: Int, val options: List<String>, val selected: Int, val hint: Int? = null, val pick: (Int) -> Unit)

@Composable
private fun Group(title: String) {
    Text(
        title,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 24.dp, bottom = 8.dp),
    )
}

@Composable
private fun Item(title: String, value: String?, valueColor: androidx.compose.ui.graphics.Color = androidx.compose.ui.graphics.Color.Unspecified, onClick: (() -> Unit)?) {
    Column(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = androidx.compose.foundation.layout.Arrangement.Center,
    ) {
        Text(title, style = MaterialTheme.typography.bodyLarge)
        if (value != null) {
            Text(value, style = MaterialTheme.typography.bodyMedium, color = valueColor.takeOrElse { MaterialTheme.colorScheme.onSurfaceVariant })
        }
    }
}

/// A line that tells, not a button.
@Composable
private fun Status(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
    )
}

@Composable
private fun Toggle(title: String, value: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .selectable(selected = value, role = Role.Switch) { onChange(!value) }
            .padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(title, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
        Switch(checked = value, onCheckedChange = null)
    }
}
