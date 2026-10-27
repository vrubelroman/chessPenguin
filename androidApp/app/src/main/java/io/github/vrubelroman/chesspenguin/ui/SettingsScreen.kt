package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.material.icons.rounded.Info
import androidx.compose.material.icons.rounded.SdCard
import androidx.compose.material.icons.rounded.DeveloperBoard
import androidx.compose.material.icons.rounded.Timer
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.automirrored.rounded.MenuBook
import kotlinx.coroutines.launch
import androidx.compose.material.icons.rounded.Memory
import androidx.compose.material.icons.rounded.Translate
import androidx.compose.material.icons.rounded.Palette
import androidx.compose.material.icons.rounded.GridOn
import androidx.compose.material.icons.rounded.Save
import androidx.compose.material.icons.rounded.Vibration
import androidx.compose.material.icons.automirrored.rounded.FormatListBulleted
import androidx.compose.material.icons.automirrored.rounded.Notes
import androidx.compose.material.icons.automirrored.rounded.VolumeUp
import androidx.compose.material.icons.automirrored.rounded.ArrowRightAlt
import androidx.compose.material.icons.rounded.Animation
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.size
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.background
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
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import io.github.vrubelroman.chesspenguin.BuildConfig
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.LANGUAGES
import io.github.vrubelroman.chesspenguin.data.Language
import io.github.vrubelroman.chesspenguin.data.NetProblem
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
        // Only a language with letters of its own (not English, Uzbek, Kazakh; move-notation.md §2.1).
        if (localLetters(lang) != null) add(Notation.Local to stringResource(R.string.notation_local))
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
            Cards {
            val notationNow = notationNames.firstOrNull { it.first == prefs.notation } ?: notationNames.first()
            Item(stringResource(R.string.notation), notationNow.second, icon = Icons.AutoMirrored.Rounded.Notes) {
                choice = Choice(
                    R.string.notation,
                    notationNames.map { it.second },
                    notationNames.indexOf(notationNow),
                    hint = R.string.notation_hint,
                ) { i -> set { it.copy(notation = notationNames[i].first) } }
            }

            Toggle(stringResource(R.string.animation), prefs.animation, Icons.Rounded.Animation) { v -> set { it.copy(animation = v) } }
            Toggle(stringResource(R.string.best_arrow), prefs.bestArrow, Icons.AutoMirrored.Rounded.ArrowRightAlt) { v -> set { it.copy(bestArrow = v) } }
            Toggle(stringResource(R.string.sound), prefs.sound, Icons.AutoMirrored.Rounded.VolumeUp) { v -> set { it.copy(sound = v) } }
            Toggle(stringResource(R.string.vibration), prefs.vibration, Icons.Rounded.Vibration) { v -> set { it.copy(vibration = v) } }
            Toggle(stringResource(R.string.record_games), prefs.recordGames, Icons.Rounded.Save) { v -> set { it.copy(recordGames = v) } }
            }

            Group(stringResource(R.string.group_look))
            Cards {
            Item(stringResource(R.string.board_and_pieces), boardLabel(prefs.board) + " · " + prefs.pieces.dir, icon = Icons.Rounded.GridOn, onClick = onBoardStyle)
            Item(stringResource(R.string.theme), themeNames.first { it.first == prefs.theme }.second, icon = Icons.Rounded.Palette) {
                choice = Choice(R.string.theme, themeNames.map { it.second }, themeNames.indexOfFirst { it.first == prefs.theme }) { i ->
                    set { it.copy(theme = themeNames[i].first) }
                }
            }
            val context = LocalContext.current
            val languageNames = LANGUAGES.map {
                if (it.isEmpty()) stringResource(R.string.theme_system) else Locale.forLanguageTag(it).let { l -> l.getDisplayLanguage(l).replaceFirstChar(Char::uppercase) }
            }
            val language = remember { Language.get(context) }
            Item(stringResource(R.string.language), languageNames[LANGUAGES.indexOf(language).coerceAtLeast(0)], icon = Icons.Rounded.Translate) {
                choice = Choice(R.string.language, languageNames, LANGUAGES.indexOf(language).coerceAtLeast(0)) { i ->
                    if (Language.set(context, LANGUAGES[i])) (context as? Activity)?.recreate()
                }
            }
            }

            Group(stringResource(R.string.group_engine))
            Cards {
            when {
                files.builtIn -> Status(stringResource(R.string.engine_built_in), Icons.Rounded.Memory)
                net == NetState.Ready -> {
                    Status(stringResource(R.string.engine_downloaded), Icons.Rounded.Memory)
                    Item(stringResource(R.string.engine_delete, size), null, icon = Icons.Rounded.Delete) { deleting = true }
                }
                net is NetState.Loading -> {
                    val l = net as NetState.Loading
                    Item(stringResource(R.string.engine_downloading, (l.done / 1_000_000).toInt(), (l.total / 1_000_000).toInt()), null, icon = Icons.Rounded.Memory) { sheet = true }
                }
                // The same sheet as in a game: the size, mobile data, cancel (§9).
                else -> Item(stringResource(R.string.engine_missing, size), null, icon = Icons.Rounded.Memory) { sheet = true }
            }
            // Rodent IV's opening books (android-rodent.md §7.2): the engine itself is in the app.
            RodentBooks(snackbar)
            Item(stringResource(R.string.lines), "${prefs.engineLines}", icon = Icons.AutoMirrored.Rounded.FormatListBulleted) {
                choice = Choice(R.string.lines, listOf("1", "2", "3"), prefs.engineLines - 1) { i -> set { it.copy(engineLines = i + 1) } }
            }
            Item(stringResource(R.string.time_per_position), secondsNames.firstOrNull { it.first == prefs.engineSeconds }?.second ?: "", icon = Icons.Rounded.Timer) {
                choice = Choice(R.string.time_per_position, secondsNames.map { it.second }, secondsNames.indexOfFirst { it.first == prefs.engineSeconds }) { i ->
                    set { it.copy(engineSeconds = secondsNames[i].first) }
                }
            }
            Item(stringResource(R.string.threads), "${prefs.engineThreads}", icon = Icons.Rounded.DeveloperBoard) {
                choice = Choice(R.string.threads, (1..cores).map { "$it" }, prefs.engineThreads - 1) { i -> set { it.copy(engineThreads = i + 1) } }
            }
            val hashes = listOf(16, 32, 64, 128, 256)
            val hashNames = hashes.map { stringResource(R.string.megabytes, it) }
            Item(stringResource(R.string.memory), stringResource(R.string.megabytes, prefs.engineHashMb), icon = Icons.Rounded.SdCard) {
                choice = Choice(R.string.memory, hashNames, hashes.indexOf(prefs.engineHashMb)) { i -> set { it.copy(engineHashMb = hashes[i]) } }
            }
            }
            Text(
                stringResource(R.string.engine_battery),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
            )
            Spacer(Modifier.height(16.dp))
            // A newer version of the app, even with its card hidden (android-ui.md §25.5).
            Cards {
                Item(stringResource(R.string.about), aboutUpdateLine(), valueColor = MaterialTheme.colorScheme.primary, icon = Icons.Rounded.Info, onClick = onAbout)
            }
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

/// «Rodent IV — opening books»: downloaded or not, with what is left to download;
/// download, or delete (not while a game against Rodent goes on); the progress and ✕
/// while downloading; why it failed, and «Try again».
@Composable
private fun RodentBooks(snackbar: androidx.compose.material3.SnackbarHostState) {
    val app = app()
    val books = app.rodentFiles.books
    val state by books.state.collectAsState()
    val game by app.maiaGames.current.collectAsState()
    val playing = game?.let { !it.over && it.rival == io.github.vrubelroman.chesspenguin.data.Opponent.Rodent } == true
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    val deletedText = stringResource(R.string.books_deleted)
    val deleteText = stringResource(R.string.books_delete_cd)
    fun mb(bytes: Long) = ((bytes + 999_999) / 1_000_000).toInt()
    // The books checked so far stay: what is left is offered.
    val left = remember(state) { books.remaining() }
    Row(
        Modifier.card().heightIn(min = 56.dp).padding(start = 16.dp, end = 4.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ItemIcon(Icons.AutoMirrored.Rounded.MenuBook)
        Column(Modifier.weight(1f)) {
            Text(stringResource(R.string.books_settings), style = MaterialTheme.typography.bodyLarge)
            when (val s = state) {
                is NetState.Loading -> {
                    androidx.compose.material3.LinearProgressIndicator(
                        progress = { s.done.toFloat() / s.total.coerceAtLeast(1) },
                        modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
                    )
                    Text(
                        stringResource(R.string.downloaded_of, mb(s.done), mb(s.total)),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                is NetState.Failed -> Text(
                    when (s.problem) {
                        NetProblem.NoInternet -> stringResource(R.string.no_internet)
                        NetProblem.Interrupted -> stringResource(R.string.interrupted)
                        NetProblem.NoSpace -> stringResource(R.string.no_space, mb(left))
                        NetProblem.Corrupt -> stringResource(R.string.corrupt)
                    },
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.error,
                )
                NetState.Ready -> Text(
                    stringResource(R.string.books_downloaded_mb, mb(books.size)),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                NetState.Missing -> Text(
                    stringResource(R.string.books_missing_mb, mb(left)),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        when (state) {
            is NetState.Loading -> IconButton(onClick = { books.cancel() }) {
                Icon(Icons.Rounded.Close, stringResource(R.string.cancel_download))
            }
            NetState.Ready -> TextButton(
                enabled = !playing,
                modifier = Modifier.semantics { contentDescription = deleteText },
                onClick = {
                    books.delete()
                    scope.launch {
                        snackbar.currentSnackbarData?.dismiss()
                        snackbar.showSnackbar(deletedText)
                    }
                },
            ) { Text(stringResource(R.string.delete)) }
            is NetState.Failed -> TextButton(onClick = { books.download() }) { Text(stringResource(R.string.play_retry)) }
            NetState.Missing -> TextButton(onClick = { books.download() }) { Text(stringResource(R.string.download)) }
        }
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

/// A group's items as cards (android-lists.md §6): 16 dp corners outside, 4 dp between
/// them, 2 dp of the background between cards.
@Composable
private fun Cards(content: @Composable androidx.compose.foundation.layout.ColumnScope.() -> Unit) {
    Column(
        Modifier.padding(horizontal = 12.dp).clip(RoundedCornerShape(16.dp)),
        verticalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(2.dp),
        content = content,
    )
}

/// One card of a group.
@Composable
private fun Modifier.card(): Modifier = fillMaxWidth().clip(RoundedCornerShape(4.dp)).background(MaterialTheme.colorScheme.surfaceContainerLow)

/// An item's icon: 24 dp, then 16 dp to its text.
@Composable
private fun ItemIcon(icon: androidx.compose.ui.graphics.vector.ImageVector?) {
    if (icon == null) return
    Icon(icon, null, tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(24.dp))
    Spacer(Modifier.width(16.dp))
}

@Composable
private fun Item(
    title: String,
    value: String?,
    valueColor: androidx.compose.ui.graphics.Color = androidx.compose.ui.graphics.Color.Unspecified,
    icon: androidx.compose.ui.graphics.vector.ImageVector? = null,
    onClick: (() -> Unit)?,
) {
    Row(
        Modifier
            .card()
            .heightIn(min = 56.dp)
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ItemIcon(icon)
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            if (value != null) {
                Text(value, style = MaterialTheme.typography.bodyMedium, color = valueColor.takeOrElse { MaterialTheme.colorScheme.onSurfaceVariant })
            }
        }
    }
}

/// A line that tells, not a button.
@Composable
private fun Status(text: String, icon: androidx.compose.ui.graphics.vector.ImageVector? = null) {
    Row(Modifier.card().heightIn(min = 56.dp).padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        ItemIcon(icon)
        Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun Toggle(title: String, value: Boolean, icon: androidx.compose.ui.graphics.vector.ImageVector? = null, onChange: (Boolean) -> Unit) {
    Row(
        Modifier
            .card()
            .heightIn(min = 56.dp)
            .selectable(selected = value, role = Role.Switch) { onChange(!value) }
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ItemIcon(icon)
        Text(title, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
        Switch(checked = value, onCheckedChange = null)
    }
}
