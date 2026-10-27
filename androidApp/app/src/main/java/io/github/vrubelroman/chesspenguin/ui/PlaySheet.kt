package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowRight
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Person
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExposedDropdownMenuAnchorType
import androidx.compose.material3.ExposedDropdownMenuBox
import androidx.compose.material3.ExposedDropdownMenuDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.ScaffoldDefaults
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.ui.draw.clip
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.NetProblem
import io.github.vrubelroman.chesspenguin.data.NetState
import io.github.vrubelroman.chesspenguin.data.Opponent
import io.github.vrubelroman.chesspenguin.data.PlayMeta
import io.github.vrubelroman.chesspenguin.theme.Tabular
import uniffi.mobile_api.RodentStyle
import uniffi.mobile_api.maiaRange
import uniffi.mobile_api.rodentRange
import uniffi.mobile_api.rodentStyles

/// The position a game against the computer starts from (android-play.md §31.6): drawn
/// small, with where it comes from; the side to move plays by default.
data class PlayFrom(val fen: String, val label: String, val whiteToMove: Boolean)

/// What the sheet starts: the opponent (Rodent in `style`), its strength, the user's
/// colour and name.
data class PlayChoice(val rival: Opponent, val strength: Int, val style: String, val userWhite: Boolean, val name: String) {
    /// The game's description; `floor`: the half-moves it goes on from.
    fun meta(floor: Int) = PlayMeta(strength, userWhite, name, floor, rival = rival, style = if (rival == Opponent.Rodent) style else "")
}

/// «Play against the computer» (android-play.md §31.1, android-rodent.md §2): the
/// opponent — Maia or Rodent IV with its character — its strength on its own scale, the
/// colour, the name. What the opponent needs is downloaded here the first time (Maia's
/// file; Rodent's opening books, or «Without book»). The state line and the button stay
/// at the bottom; the rest scrolls.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PlaySheet(from: PlayFrom?, onPlay: (PlayChoice) -> Unit, onDismiss: () -> Unit) {
    val app = app()
    val prefs by app.settings.prefs.collectAsState()
    val maiaFiles = app.maiaFiles.download
    val maiaState by maiaFiles.state.collectAsState()
    val books = app.rodentFiles.books
    val booksState by books.state.collectAsState()
    val styles = remember { rodentStyles() }
    var rival by remember { mutableStateOf(prefs.opponent) }
    val rodent = rival == Opponent.Rodent
    // Each opponent's strength on its own scale, remembered apart (§4).
    val maiaRange = remember { maiaRange().map { it.toInt() } }
    val rodentRange = remember { rodentRange().map { it.toInt() } }
    var maiaLevel by remember { mutableFloatStateOf(prefs.maiaStrength.coerceIn(maiaRange[0], maiaRange[1]).toFloat()) }
    var rodentLevel by remember { mutableFloatStateOf(prefs.rodentStrength.coerceIn(rodentRange[0], rodentRange[1]).toFloat()) }
    // The character: kept at once when chosen (§7.1); an unknown one is Tal.
    val style = prefs.rodentPersonality.takeIf { id -> styles.any { it.id == id } } ?: DEFAULT_RODENT_STYLE
    var picking by remember { mutableStateOf(false) }
    // From a position: the side to move by default, not remembered (play-from-position.md §2).
    var color by remember { mutableIntStateOf(if (from != null) (if (from.whiteToMove) 1 else -1) else prefs.playColor) }
    var name by remember { mutableStateOf(prefs.yourName) }
    // «Download and play»: the game starts once the files are checked, if this opponent
    // is still the one chosen.
    var playAfter by remember { mutableStateOf(false) }
    val range = if (rodent) rodentRange else maiaRange
    val strength = if (rodent) rodentLevel else maiaLevel
    val level = (strength / 50f).toInt() * 50
    // From a position Rodent needs no book (§1.5); Maia always needs her file.
    val state = if (rodent) booksState else maiaState
    // From a position Rodent plays at once, whatever its books (§1.5, question 4).
    val noBookNeeded = rodent && from != null
    val ready = state == NetState.Ready || noBookNeeded
    fun play() {
        val white = when (color) {
            1 -> true
            -1 -> false
            else -> kotlin.random.Random.nextBoolean()
        }
        app.settings.update { p ->
            val q = p.copy(opponent = rival, yourName = name.trim(), playColor = if (from != null) p.playColor else color)
            if (rodent) q.copy(rodentStrength = level) else q.copy(maiaStrength = level)
        }
        onPlay(PlayChoice(rival, level, style, white, name.trim()))
    }
    // A failure of an earlier attempt is not this sheet's: the download is offered anew.
    LaunchedEffect(Unit) { books.forgetFailure() }
    LaunchedEffect(state, rival) {
        if (playAfter && state == NetState.Ready) {
            playAfter = false
            play()
        }
    }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        val scroll = rememberScrollState()
        Column(Modifier.fillMaxWidth()) {
            Column(Modifier.weight(1f, fill = false).verticalScroll(scroll).padding(horizontal = 24.dp)) {
                Text(stringResource(R.string.play_title), style = MaterialTheme.typography.titleLarge)
                if (from != null) {
                    Row(Modifier.padding(top = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                        MiniBoard(from.fen, flipped = color == -1, size = 96.dp)
                        Text(from.label, Modifier.padding(start = 16.dp), style = MaterialTheme.typography.bodyMedium, maxLines = 4, overflow = TextOverflow.Ellipsis)
                    }
                }
                Spacer(Modifier.height(16.dp))
                // Who first, then how strong: the scale is the opponent's (§2.1).
                Row(Modifier.fillMaxWidth().selectableGroup(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OpponentCard("Maia", stringResource(R.string.opp_maia_sub), !rodent, Modifier.weight(1f)) {
                        if (rodent) playAfter = false
                        rival = Opponent.Maia
                    }
                    OpponentCard("Rodent IV", stringResource(R.string.opp_rodent_sub), rodent, Modifier.weight(1f)) {
                        if (!rodent) playAfter = false
                        rival = Opponent.Rodent
                        // The first time, the list opens by itself with Tal suggested (§3.4).
                        if (prefs.rodentPersonality.isEmpty()) picking = true
                    }
                }
                Spacer(Modifier.height(8.dp))
                CharacterRow(if (rodent) styles.firstOrNull { it.id == style } else null) { picking = true }
                Spacer(Modifier.height(8.dp))
                Text(
                    stringResource(R.string.strength_n, level.toString()),
                    style = MaterialTheme.typography.titleMedium.merge(Tabular),
                    fontWeight = FontWeight.Medium,
                )
                val strengthLabel = stringResource(R.string.strength)
                val strengthState = stringResource(R.string.strength_range, level.toString(), range[0].toString(), range[1].toString())
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("${range[0]}", style = MaterialTheme.typography.bodySmall.merge(Tabular), color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Slider(
                        value = strength,
                        onValueChange = { if (rodent) rodentLevel = it else maiaLevel = it },
                        valueRange = range[0].toFloat()..range[1].toFloat(),
                        modifier = Modifier.weight(1f).padding(horizontal = 8.dp).semantics {
                            contentDescription = strengthLabel
                            stateDescription = strengthState
                        },
                    )
                    Text("${range[1]}", style = MaterialTheme.typography.bodySmall.merge(Tabular), color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                // Always two lines: the sheet does not jump when the opponent changes. It
                // changes only with the opponent: TalkBack says it then, not at each step of
                // the slider.
                Text(
                    stringResource(if (rodent) R.string.rodent_measured else R.string.play_measured),
                    Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    minLines = 2,
                )
                Spacer(Modifier.height(16.dp))
                Text(stringResource(R.string.play_colour), style = MaterialTheme.typography.titleSmall)
                Spacer(Modifier.height(8.dp))
                val options = listOf(1 to R.string.color_white, 0 to R.string.color_random, -1 to R.string.color_black)
                SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                    options.forEachIndexed { i, (v, label) ->
                        SegmentedButton(selected = color == v, onClick = { color = v }, shape = SegmentedButtonDefaults.itemShape(i, options.size)) {
                            Text(stringResource(label))
                        }
                    }
                }
                Spacer(Modifier.height(12.dp))
                NameField(name, prefs.recordGames) { name = it }
                Spacer(Modifier.height(12.dp))
            }
            // The footer: what the chosen opponent needs, and the button (§2.4). A line
            // over it while the content goes on under it.
            HorizontalDivider(color = if (scroll.canScrollForward) MaterialTheme.colorScheme.outlineVariant else Color.Transparent)
            Column(Modifier.padding(start = 24.dp, end = 24.dp, top = 8.dp, bottom = 24.dp)) {
                // At least 40 dp (it grows with a large font). Only the lines that change with
                // the kind of state are read out, not every step of the progress.
                Row(Modifier.fillMaxWidth().heightIn(min = 40.dp), verticalAlignment = Alignment.CenterVertically) {
                    val noBookText = stringResource(R.string.books_without_cd)
                    val noBook = @Composable {
                        TextButton(onClick = {
                            playAfter = false
                            play()
                        }, modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = noBookText }) {
                            Text(stringResource(R.string.books_without), maxLines = 1)
                        }
                    }
                    // What is left to download (the books checked so far stay).
                    val booksLeft = remember(booksState) { books.remaining() }
                    val left = if (rodent) booksLeft else maiaFiles.size
                    when (val s = state) {
                        NetState.Ready -> StateText(stringResource(if (rodent) R.string.books_ready else R.string.play_maia_ready))
                        NetState.Missing -> if (noBookNeeded) {
                            StateText(stringResource(R.string.books_not_needed))
                        } else if (rodent) {
                            StateText(stringResource(R.string.books_offer, megabytes(left)), Modifier.weight(1f))
                            noBook()
                        } else {
                            StateText(stringResource(R.string.play_download_note, megabytes(left)))
                        }
                        is NetState.Loading -> {
                            val what = stringResource(R.string.online_downloading, if (rodent) stringResource(R.string.books_notification) else "Maia")
                            Column(
                                Modifier.weight(1f).clearAndSetSemantics {
                                    contentDescription = what
                                    progressBarRangeInfo = ProgressBarRangeInfo(s.done.toFloat() / s.total.coerceAtLeast(1), 0f..1f)
                                },
                            ) {
                                LinearProgressIndicator(progress = { s.done.toFloat() / s.total.coerceAtLeast(1) }, modifier = Modifier.fillMaxWidth())
                                Text(stringResource(R.string.downloaded_of, megabytes(s.done), megabytes(s.total)), style = MaterialTheme.typography.bodySmall.merge(Tabular))
                            }
                            IconButton(onClick = {
                                playAfter = false
                                if (rodent) books.cancel() else maiaFiles.cancel()
                            }) { Icon(Icons.Rounded.Close, stringResource(R.string.cancel_download)) }
                        }
                        // From a position a failed download does not matter to Rodent.
                        is NetState.Failed -> if (noBookNeeded) {
                            StateText(stringResource(R.string.books_not_needed))
                        } else {
                            Text(
                                when (s.problem) {
                                    NetProblem.NoInternet -> stringResource(R.string.no_internet)
                                    NetProblem.Interrupted -> stringResource(R.string.interrupted)
                                    NetProblem.NoSpace -> stringResource(R.string.no_space, megabytes(left))
                                    NetProblem.Corrupt -> stringResource(R.string.corrupt)
                                },
                                Modifier.weight(1f).semantics { liveRegion = LiveRegionMode.Polite },
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.error,
                            )
                            // Rodent plays without its books (§5.5); from a position it needs none.
                            if (rodent) noBook()
                        }
                    }
                }
                Spacer(Modifier.height(8.dp))
                val loading = state is NetState.Loading
                Button(
                    onClick = {
                        if (ready) {
                            play()
                        } else {
                            playAfter = true
                            if (rodent) books.download() else maiaFiles.download()
                        }
                    },
                    enabled = !loading || noBookNeeded,
                    modifier = Modifier.fillMaxWidth().height(56.dp),
                ) {
                    Text(
                        stringResource(
                            when {
                                ready || loading -> R.string.play_play
                                state is NetState.Failed -> R.string.play_retry
                                else -> R.string.play_download_and_play
                            },
                        ),
                    )
                }
            }
        }
    }
    if (picking) {
        // Closed without a choice the first time: Tal stays, and the list does not open
        // by itself again (§3.4).
        StylePicker(styles, style, onPick = { id ->
            picking = false
            app.settings.update { it.copy(rodentPersonality = id) }
        }, onDismiss = {
            picking = false
            if (prefs.rodentPersonality.isEmpty()) app.settings.update { it.copy(rodentPersonality = style) }
        })
    }
}

@Composable
private fun StateText(text: String, modifier: Modifier = Modifier) {
    // Not cut: with a large font the footer grows (§8.2).
    Text(text, modifier.semantics { liveRegion = LiveRegionMode.Polite }, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

/// One of the two opponents (§2.2): chosen, it is filled and ticked (not by colour alone).
@Composable
private fun OpponentCard(title: String, sub: String, selected: Boolean, modifier: Modifier, onClick: () -> Unit) {
    OutlinedCard(
        modifier = modifier.heightIn(min = 64.dp).selectable(selected = selected, role = Role.RadioButton, onClick = onClick),
        shape = RoundedCornerShape(12.dp),
        colors = CardDefaults.outlinedCardColors(
            containerColor = if (selected) MaterialTheme.colorScheme.secondaryContainer else MaterialTheme.colorScheme.surface,
        ),
        border = BorderStroke(if (selected) 2.dp else 1.dp, if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant),
    ) {
        Column(Modifier.padding(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (selected) {
                    Icon(Icons.Rounded.Check, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.primary)
                    Spacer(Modifier.width(4.dp))
                }
                Text(title, style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Medium, maxLines = 1)
            }
            Text(sub, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, minLines = 2, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
    }
}

/// The character's line (§2.3): Rodent's character, a tap opens the list; for Maia the
/// same line, inactive («Like a human»), so that the sheet keeps its height.
@Composable
private fun CharacterRow(style: RodentStyle?, onClick: () -> Unit) {
    val title = style?.let { styleTitle(it.id) } ?: stringResource(R.string.like_human)
    val about = style?.let { s -> RODENT_TEXTS[s.id]?.let { stringResource(it.second) } ?: s.about } ?: stringResource(R.string.maia_no_style)
    val active = style != null
    val described = if (active) stringResource(R.string.character_cd, title) + ". " + about else "$title. $about"
    val change = stringResource(R.string.pick_change)
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 80.dp)
            .background(MaterialTheme.colorScheme.surfaceContainerHigh, RoundedCornerShape(12.dp))
            .then(if (active) Modifier.clickable(onClickLabel = change, role = Role.Button, onClick = onClick) else Modifier)
            .clearAndSetSemantics { contentDescription = described }
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val dim = if (active) 1f else 0.38f
        if (style != null) {
            Initial(title, style.player)
        } else {
            Box(Modifier.size(40.dp).background(MaterialTheme.colorScheme.surfaceContainerHighest, CircleShape), contentAlignment = Alignment.Center) {
                Icon(Icons.Rounded.Person, null, Modifier.size(24.dp), tint = MaterialTheme.colorScheme.onSurface.copy(alpha = dim))
            }
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Medium, color = MaterialTheme.colorScheme.onSurface.copy(alpha = dim), maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(about, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = dim), minLines = 2, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
        if (active) Icon(Icons.AutoMirrored.Rounded.KeyboardArrowRight, null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/// A character's initial in a circle (§3.2): the players in one colour, the characters
/// in another. Decoration: not read out.
@Composable
private fun Initial(title: String, player: Boolean) {
    val (back, fore) = if (player) {
        MaterialTheme.colorScheme.tertiaryContainer to MaterialTheme.colorScheme.onTertiaryContainer
    } else {
        MaterialTheme.colorScheme.secondaryContainer to MaterialTheme.colorScheme.onSecondaryContainer
    }
    Box(Modifier.size(40.dp).background(back, CircleShape), contentAlignment = Alignment.Center) {
        Text(title.take(1), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Medium, color = fore)
    }
}

/// «Who to play» (§3): Rodent IV's characters in two groups, each in the alphabet of the
/// app's language; a tap chooses and closes. Full screen on a phone, a dialog on a tablet.
@OptIn(ExperimentalMaterial3Api::class, ExperimentalFoundationApi::class)
@Composable
private fun StylePicker(styles: List<RodentStyle>, selected: String, onPick: (String) -> Unit, onDismiss: () -> Unit) {
    val wide = with(LocalDensity.current) { LocalWindowInfo.current.containerSize.width.toDp() } >= 600.dp
    val locale = LocalConfiguration.current.locales[0]
    val titled = styles.map { it to styleTitle(it.id) }
    val collator = remember(locale) { java.text.Collator.getInstance(locale) }
    val groups = listOf(R.string.pick_players to true, R.string.pick_characters to false).map { (heading, player) ->
        heading to titled.filter { it.first.player == player }.sortedWith { a, b -> collator.compare(a.second, b.second) }
    }
    // Opened with the chosen one in sight: the row above it just under the group's heading
    // (which stays on top of the list), so that neither is cut.
    val index = run {
        var i = 0
        for ((_, rows) in groups) {
            i++ // the group's heading
            val at = rows.indexOfFirst { it.first.id == selected }
            if (at >= 0) return@run i + at
            i += rows.size
        }
        0
    }
    val list = rememberLazyListState()
    val headerPx = with(LocalDensity.current) { 48.dp.roundToPx() }
    LaunchedEffect(Unit) {
        // A negative offset: the row above starts below the sticky heading.
        if (index >= 2) list.scrollToItem(index - 1, -headerPx)
    }
    // Full screen: edge to edge, the bars' insets inside. A dialog: in the window between
    // the system bars, clipped to its corners, no insets of its own.
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = wide)) {
        val frame = if (wide) {
            Modifier.widthIn(max = 560.dp).fillMaxWidth().fillMaxHeight(0.9f).clip(RoundedCornerShape(28.dp))
        } else {
            Modifier.fillMaxSize()
        }
        val insets = if (wide) WindowInsets(0) else ScaffoldDefaults.contentWindowInsets
        Scaffold(
            modifier = frame,
            containerColor = MaterialTheme.colorScheme.surface,
            contentWindowInsets = insets,
            topBar = {
                Column {
                    TopAppBar(
                        windowInsets = if (wide) WindowInsets(0) else TopAppBarDefaults.windowInsets,
                        navigationIcon = { IconButton(onClick = onDismiss) { Icon(Icons.AutoMirrored.Rounded.ArrowBack, stringResource(R.string.back)) } },
                        title = { Text(stringResource(R.string.pick_title)) },
                    )
                    // Fixed under the bar: what the list is for, wherever it is scrolled.
                    Text(
                        stringResource(R.string.pick_intro),
                        Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            },
        ) { padding ->
            LazyColumn(Modifier.fillMaxSize().padding(padding), state = list) {
                groups.forEach { (heading, rows) ->
                    stickyHeader(key = heading) {
                        Surface(color = MaterialTheme.colorScheme.surface) {
                            Text(
                                stringResource(heading),
                                Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(horizontal = 16.dp, vertical = 14.dp).semantics { heading() },
                                style = MaterialTheme.typography.titleSmall,
                                color = MaterialTheme.colorScheme.primary,
                            )
                        }
                    }
                    items(rows, key = { it.first.id }) { (st, title) ->
                        val on = st.id == selected
                        val about = RODENT_TEXTS[st.id]?.let { stringResource(it.second) } ?: st.about
                        val described = stringResource(if (st.player) R.string.pick_player_cd else R.string.pick_character_cd, title) + ". " + about
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .heightIn(min = 72.dp)
                                .background(if (on) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent)
                                .selectable(selected = on, role = Role.RadioButton) { onPick(st.id) }
                                // One reading: the name, the group, what it does; not the texts again.
                                .clearAndSetSemantics {
                                    contentDescription = described
                                    this.selected = on
                                    role = Role.RadioButton
                                    onClick { onPick(st.id); true }
                                }
                                .padding(horizontal = 16.dp, vertical = 8.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Initial(title, st.player)
                            Spacer(Modifier.width(16.dp))
                            Column(Modifier.weight(1f)) {
                                Text(title, style = MaterialTheme.typography.bodyLarge)
                                Text(about, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                            if (on) Icon(Icons.Rounded.Check, null, Modifier.padding(start = 8.dp), tint = MaterialTheme.colorScheme.primary)
                        }
                    }
                }
            }
        }
    }
}

/// The name, with the players of this phone to pick from; under it, where the games go
/// (player-games.md §6).
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun NameField(name: String, record: Boolean, onName: (String) -> Unit) {
    val app = app()
    val players = remember { app.playerGames.players() }
    var picking by remember { mutableStateOf(false) }
    val line = if (!record) {
        stringResource(R.string.player_line_off)
    } else {
        val db = app.playerGames.database(name)
        when {
            app.playerGames.tidy(name).isEmpty() -> stringResource(R.string.player_line_noname, db?.name ?: app.playerGames.dbName(""))
            db != null -> stringResource(R.string.player_line_existing, db.name) + " · " +
                androidx.compose.ui.res.pluralStringResource(R.plurals.games_count, db.games ?: 0, groupDigits(db.games ?: 0))
            else -> stringResource(R.string.player_line_new, app.playerGames.dbName(name))
        }
    }
    // The players whose names begin with what is typed (none once it is one of them).
    val typed = app.playerGames.tidy(name).lowercase()
    val matching = players.filter { it.lowercase().startsWith(typed) && it.lowercase() != typed }
    // Opened by typing or by its arrow, not by a tap in the field (it would cover «Play»).
    ExposedDropdownMenuBox(expanded = picking && matching.isNotEmpty(), onExpandedChange = { if (!it) picking = false }) {
        OutlinedTextField(
            name,
            {
                onName(it)
                picking = it.isNotBlank()
            },
            Modifier.fillMaxWidth().menuAnchor(ExposedDropdownMenuAnchorType.PrimaryEditable),
            label = { Text(stringResource(R.string.play_your_name)) },
            supportingText = { Text(line) },
            trailingIcon = {
                if (matching.isNotEmpty()) {
                    IconButton(onClick = { picking = !picking }) { ExposedDropdownMenuDefaults.TrailingIcon(expanded = picking) }
                }
            },
            singleLine = true,
        )
        ExposedDropdownMenu(expanded = picking && matching.isNotEmpty(), onDismissRequest = { picking = false }) {
            matching.forEach { p ->
                DropdownMenuItem(text = { Text(p) }, onClick = {
                    onName(p)
                    picking = false
                })
            }
        }
    }
}

/// A small board of a position (no touches, no coordinates).
@Composable
fun MiniBoard(fen: String, flipped: Boolean, size: androidx.compose.ui.unit.Dp) {
    val app = app()
    val prefs by app.settings.prefs.collectAsState()
    val context = androidx.compose.ui.platform.LocalContext.current
    val pieces = remember { Pieces(context) }
    Board(
        fen = fen,
        flipped = flipped,
        lastFrom = null,
        lastTo = null,
        pieces = pieces,
        theme = prefs.board,
        set = prefs.pieces,
        description = "",
        animated = false,
        modifier = Modifier.width(size),
        coordinates = false,
    )
}

private fun megabytes(bytes: Long) = ((bytes + 999_999) / 1_000_000).toInt()
