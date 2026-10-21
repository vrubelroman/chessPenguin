package io.github.vrubelroman.chesspenguin.ui

import android.content.Context
import android.view.HapticFeedbackConstants
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.ArrowLeft
import androidx.compose.material.icons.automirrored.rounded.ArrowRight
import androidx.compose.material.icons.outlined.Memory
import androidx.compose.material.icons.rounded.ChevronLeft
import androidx.compose.material.icons.rounded.ChevronRight
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.FirstPage
import androidx.compose.material.icons.rounded.LastPage
import androidx.compose.material.icons.rounded.Memory
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material.icons.rounded.SwapVert
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.disabled
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.EngineSession
import io.github.vrubelroman.chesspenguin.data.NetState
import io.github.vrubelroman.chesspenguin.data.enginePosition
import io.github.vrubelroman.chesspenguin.theme.Tabular
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.mobile_api.EngineLine
import uniffi.mobile_api.GameData
import uniffi.mobile_api.MoveNode

/// An engine line shown on the board (§8.3): the game itself does not change.
private data class Preview(val line: EngineLine, val step: Int)

/// One game: the board on top, the notation under it, the move buttons at the bottom
/// under the thumb; ‹ › page the games of the list (android-ui.md §7).
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun GameScreen(db: String, startPlace: Int, onBack: () -> Unit, onSettings: () -> Unit) {
    val app = app()
    val context = LocalContext.current
    val prefs by app.settings.prefs.collectAsState()
    var games by remember { mutableStateOf(app.shown[db]) }
    var place by rememberSaveable { mutableIntStateOf(startPlace) }
    var game by remember { mutableStateOf<GameData?>(null) }
    var failed by remember { mutableStateOf(false) }
    var current by rememberSaveable(place) { mutableIntStateOf(0) }
    var flipped by rememberSaveable { mutableStateOf(false) }
    var preview by remember { mutableStateOf<Preview?>(null) }
    LaunchedEffect(db, place) {
        preview = null
        val database = app.library.database(db).getOrNull()
        if (database == null) {
            failed = true
            return@LaunchedEffect
        }
        // After the process was restarted the list is gone: page all the games.
        val list = games ?: withContext(Dispatchers.Default) { database.find("") }.also {
            games = it
            app.shown[db] = it
        }
        val index = list.getOrNull(place) ?: return@LaunchedEffect
        game = null
        val loaded = withContext(Dispatchers.Default) { runCatching { database.game(index) } }
        failed = loaded.isFailure
        game = loaded.getOrNull()
    }
    val pieces = remember { Pieces(context) }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    val hint = stringResource(R.string.board_hint)
    val readyText = stringResource(R.string.engine_ready)
    val cancelledText = stringResource(R.string.download_cancelled)
    val total = games?.size ?: 0
    val lang = uiLanguage()
    val san = remember(prefs.notation, lang) { { s: String -> displaySan(s, prefs.notation, lang) } }
    val line = remember(prefs.notation, lang) { { s: String -> displayLine(s, prefs.notation, lang) } }

    // The engine: one process while this screen is visible and the button is on.
    val files = app.engineFiles
    val netState by files.state.collectAsState()
    val session = remember { EngineSession(files, app.scope) }
    val status by session.status.collectAsState()
    val analysis by session.analysis.collectAsState()
    var visible by remember { mutableStateOf(true) }
    var sheet by remember { mutableStateOf(false) }
    var wantEngine by remember { mutableStateOf(false) }
    var expanded by rememberSaveable { mutableStateOf(false) }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, e ->
            when (e) {
                Lifecycle.Event.ON_START -> visible = true
                // The screen went dark or the app went away: the engine quits (§8.3).
                Lifecycle.Event.ON_STOP -> {
                    visible = false
                    session.shutdown()
                }
                else -> {}
            }
        }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            session.shutdown()
        }
    }

    val g = game
    val tag = { name: String -> g?.tags?.firstOrNull { it.name == name }?.value ?: "" }
    val white = surname(tag("White"))
    val black = surname(tag("Black"))
    val resultText = g?.let { resultLine(context, it.result) } ?: ""
    val (items, where) = remember(g, resultText) { g?.let { layout(it, resultText) } ?: (emptyList<Item>() to emptyMap()) }
    val nodes = g?.nodes
    val node = nodes?.getOrNull(current)
    val notation = rememberLazyListState()
    val engineReady = netState == NetState.Ready
    val engineOn = prefs.engineOn && engineReady

    LaunchedEffect(g, current, engineOn, visible, prefs.engineLines, prefs.engineSeconds) {
        if (g == null || !visible) return@LaunchedEffect
        if (engineOn) session.show(enginePosition(g, current), prefs) else session.shutdown()
    }
    // A download started from the button turns the engine on when it is done.
    LaunchedEffect(netState) {
        if (netState == NetState.Ready && wantEngine) {
            wantEngine = false
            sheet = false
            app.settings.update { it.copy(engineOn = true) }
            if (!files.builtIn) snackbar.showSnackbar(readyText)
        }
    }

    fun buzz(edge: Boolean) {
        if (prefs.vibration) view.performHapticFeedback(if (edge) HapticFeedbackConstants.CONFIRM else HapticFeedbackConstants.CLOCK_TICK)
    }
    fun go(to: Int) {
        preview = null
        if (nodes == null || to == current || to !in nodes.indices) return
        current = to
        buzz(to == 0 || nodes[to].children.isEmpty())
    }
    fun stepPreview(by: Int) {
        val p = preview ?: return
        val next = p.step + by
        if (next < 0) {
            preview = null
        } else if (next < p.line.steps.size) {
            preview = p.copy(step = next)
            buzz(next == p.line.steps.size - 1)
        }
    }
    val canBack = preview != null || node?.parent != null
    val canForward = preview?.let { it.step + 1 < it.line.steps.size } ?: (node?.children?.isNotEmpty() == true)
    fun back() {
        if (preview != null) stepPreview(-1) else node?.parent?.let { go(it.toInt()) }
    }
    fun forward() {
        if (preview != null) stepPreview(1) else node?.children?.firstOrNull()?.let { go(it.toInt()) }
    }
    fun end() {
        preview?.let {
            preview = it.copy(step = it.line.steps.size - 1)
            return
        }
        var n = node ?: return
        while (n.children.isNotEmpty()) n = nodes[n.children[0].toInt()]
        go(n.id.toInt())
    }
    fun toggleEngine() {
        when {
            prefs.engineOn && engineReady -> app.settings.update { it.copy(engineOn = false) }
            engineReady -> app.settings.update { it.copy(engineOn = true) }
            files.builtIn -> {
                wantEngine = true
                files.prepare()
            }
            else -> sheet = true
        }
    }
    BackHandler(preview != null) { preview = null }

    Scaffold(
        topBar = {
            TopAppBar(
                navigationIcon = {
                    IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Rounded.ArrowBack, stringResource(R.string.back)) }
                },
                title = {
                    Column {
                        Text(
                            when {
                                g == null -> ""
                                black == "?" && white != "?" -> white
                                else -> "$white – $black"
                            },
                            style = MaterialTheme.typography.titleMedium,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        if (total > 0) {
                            Text(
                                joinKnown(stringResource(R.string.game_of, place + 1, total), g?.let { shortResult(it.result) } ?: ""),
                                style = MaterialTheme.typography.bodyMedium.merge(Tabular),
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                },
                actions = {
                    IconButton(onClick = { place -= 1 }, enabled = place > 0) {
                        Icon(Icons.Rounded.ChevronLeft, stringResource(R.string.previous_game))
                    }
                    IconButton(onClick = { place += 1 }, enabled = place + 1 < total) {
                        Icon(Icons.Rounded.ChevronRight, stringResource(R.string.next_game))
                    }
                    IconButton(onClick = onSettings) { Icon(Icons.Rounded.Settings, stringResource(R.string.settings)) }
                },
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(top = padding.calculateTopPadding())) {
            val p = preview
            val step = p?.let { it.line.steps[it.step] }
            val fen = step?.fen ?: node?.fen ?: START
            val description = when {
                step != null -> stringResource(R.string.board_description, step.label)
                node == null || current == 0 -> stringResource(R.string.board_start)
                else -> stringResource(R.string.board_description, (if (node.white) "${node.moveNumber}." else "${node.moveNumber}…") + node.san)
            }
            val arrows = buildList {
                if (p != null) {
                    p.line.steps.getOrNull(p.step + 1)?.let { add(BoardArrow(it.fromSquare, it.toSquare, EngineArrow)) }
                } else {
                    node?.marks?.forEach { m -> m.to?.let { add(BoardArrow(m.from, it, markColor(m.color), 0.9f)) } }
                    if (engineOn && prefs.bestArrow) {
                        analysis?.lines?.take(2)?.forEachIndexed { i, l ->
                            val f = l.fromSquare
                            val t = l.toSquare
                            if (f != null && t != null) {
                                add(BoardArrow(f, t, EngineArrow.copy(alpha = if (i == 0) 0.63f else 0.35f), if (i == 0) 1f else 0.7f))
                            }
                        }
                    }
                }
            }
            val marks = if (p == null) node?.marks?.filter { it.to == null }?.map { BoardMark(it.from, markColor(it.color)) }.orEmpty() else emptyList()
            val check = (step?.label ?: node?.san ?: "").let { it.endsWith("+") || it.endsWith("#") }
            Board(
                fen = fen,
                flipped = flipped,
                lastFrom = step?.fromSquare ?: node?.fromSquare,
                lastTo = step?.toSquare ?: node?.toSquare,
                pieces = pieces,
                description = description,
                arrows = arrows,
                marks = marks,
                check = check,
                animated = prefs.animation,
                modifier = Modifier.pointerInput(Unit) {
                    detectTapGestures {
                        if (firstBoardTouch(context)) scope.launch { snackbar.showSnackbar(hint) }
                    }
                },
            )
            if (engineOn) {
                EngineStrip(
                    status = status,
                    analysis = analysis,
                    flipped = flipped,
                    lines = prefs.engineLines,
                    expanded = expanded,
                    onExpand = { expanded = !expanded },
                    onLine = { l -> if (l.steps.isNotEmpty()) preview = Preview(l, 0) },
                    onDeeper = { session.deeper() },
                    onRestart = {
                        session.shutdown()
                        g?.let { session.show(enginePosition(it, current), prefs) }
                    },
                    notation = line,
                )
            } else if (wantEngine && netState is NetState.Loading) {
                Row(Modifier.fillMaxWidth().height(48.dp).padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    Text(stringResource(R.string.preparing_engine), Modifier.padding(start = 12.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            if (p != null) PreviewBar(p.step + 1, p.line.steps.size, onClose = { preview = null })
            Box(Modifier.weight(1f).fillMaxWidth()) {
                when {
                    g != null -> Notation(g, items, where, current, notation, san, onMove = ::go, modifier = Modifier.fillMaxSize())
                    failed -> Text(
                        stringResource(R.string.game_failed),
                        color = MaterialTheme.colorScheme.error,
                        modifier = Modifier.align(Alignment.Center),
                    )
                    else -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                }
            }
            if (p == null && node != null && nodes != null && node.children.size > 1) {
                Forks(node.children.map { nodes[it.toInt()] }, san, onMove = ::go)
            }
            HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
            Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
                Row(
                    Modifier.fillMaxWidth().navigationBarsPadding().height(72.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    EngineButton(
                        on = engineOn,
                        needsDownload = !engineReady && !files.builtIn,
                        loading = netState as? NetState.Loading,
                        modifier = Modifier.weight(1.2f),
                        onClick = ::toggleEngine,
                    )
                    BarButton(Icons.Rounded.SwapVert, stringResource(R.string.flip), true, Modifier.weight(1f)) { flipped = !flipped }
                    BarButton(Icons.Rounded.FirstPage, stringResource(R.string.to_start), canBack, Modifier.weight(1f)) {
                        if (preview != null) preview = null else go(0)
                    }
                    BarButton(Icons.AutoMirrored.Rounded.ArrowLeft, stringResource(R.string.move_back), canBack, Modifier.weight(1.5f), 44, repeat = true) { back() }
                    BarButton(Icons.AutoMirrored.Rounded.ArrowRight, stringResource(R.string.move_forward), canForward, Modifier.weight(1.5f), 44, repeat = true) { forward() }
                    BarButton(Icons.Rounded.LastPage, stringResource(R.string.to_end), canForward, Modifier.weight(1f)) { end() }
                }
            }
        }
    }
    if (sheet) {
        EngineSheet(
            state = netState,
            metered = remember(sheet) { !files.builtIn && files.metered() },
            onDownload = {
                wantEngine = true
                files.prepare()
            },
            onCancel = {
                files.cancel()
                wantEngine = false
                sheet = false
                scope.launch { snackbar.showSnackbar(cancelledText) }
            },
            onDismiss = { sheet = false },
        )
    }
}

private const val START = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"

/// The engine switch of the bottom bar, with its label (§7.4): filled and underlined
/// when on; with a download mark while the light build has no engine yet.
@Composable
private fun EngineButton(on: Boolean, needsDownload: Boolean, loading: NetState.Loading?, modifier: Modifier, onClick: () -> Unit) {
    val label = stringResource(R.string.engine)
    val action by rememberUpdatedState(onClick)
    val color = if (on) MaterialTheme.colorScheme.primary else LocalContentColor.current
    Column(
        modifier
            .fillMaxHeight()
            .semantics {
                contentDescription = label
                role = Role.Switch
                stateDescription = if (on) "on" else "off"
                onClick { action(); true }
            }
            .pointerInput(Unit) { detectTapGestures { action() } },
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Box(Modifier.size(36.dp), contentAlignment = Alignment.Center) {
            Icon(if (on) Icons.Rounded.Memory else Icons.Outlined.Memory, null, tint = color)
            if (loading != null) {
                CircularProgressIndicator(
                    progress = { loading.done.toFloat() / loading.total.coerceAtLeast(1) },
                    modifier = Modifier.size(34.dp),
                    strokeWidth = 2.dp,
                )
            } else if (needsDownload) {
                Icon(
                    Icons.Rounded.Download,
                    null,
                    tint = color,
                    modifier = Modifier.size(14.dp).align(Alignment.BottomEnd).background(MaterialTheme.colorScheme.surfaceContainer),
                )
            }
        }
        Text(label, style = MaterialTheme.typography.labelSmall, color = color)
        Box(Modifier.padding(top = 2.dp).width(24.dp).height(2.dp).background(if (on) MaterialTheme.colorScheme.primary else Color.Transparent))
    }
}

/// The row of continuations at a fork (§7.5): the line's own move first and filled.
@Composable
private fun Forks(moves: List<MoveNode>, san: (String) -> AnnotatedString, onMove: (Int) -> Unit) {
    Row(
        Modifier.fillMaxWidth().height(48.dp).horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(stringResource(R.string.next_moves), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
        moves.forEachIndexed { i, m ->
            FilterChip(
                selected = i == 0,
                onClick = { onMove(m.id.toInt()) },
                label = { Text(AnnotatedString(if (m.white) "${m.moveNumber}." else "${m.moveNumber}…") + san(m.san)) },
            )
        }
    }
}

/// A button of the bottom bar. With `repeat`, holding it steps again after 400 ms and
/// then every 120 ms (§7.4).
@Composable
private fun BarButton(
    icon: ImageVector,
    label: String,
    enabled: Boolean,
    modifier: Modifier,
    iconSize: Int = 24,
    repeat: Boolean = false,
    onClick: () -> Unit,
) {
    val action by rememberUpdatedState(onClick)
    val on by rememberUpdatedState(enabled)
    val color = if (enabled) LocalContentColor.current else MaterialTheme.colorScheme.onSurface.copy(alpha = 0.38f)
    Box(
        modifier
            .fillMaxHeight()
            .semantics {
                contentDescription = label
                role = Role.Button
                if (!enabled) disabled()
                onClick { action(); true }
            }
            .pointerInput(repeat) {
                detectTapGestures(onPress = {
                    if (!on) return@detectTapGestures
                    action()
                    if (!repeat) return@detectTapGestures
                    coroutineScope {
                        val job = launch {
                            delay(400)
                            while (on) {
                                action()
                                delay(120)
                            }
                        }
                        tryAwaitRelease()
                        job.cancel()
                    }
                })
            },
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, null, tint = color, modifier = Modifier.size(iconSize.dp))
    }
}

/// "Carlsen, Magnus" → "Carlsen"; "Magnus Carlsen" stays as it is.
private fun surname(name: String): String = name.substringBefore(',').trim().ifEmpty { "?" }

/// "1-0 · White won" for the end of the notation.
private fun resultLine(context: Context, result: String): String = when (result) {
    "1-0" -> "1-0 · " + context.getString(R.string.white_won)
    "0-1" -> "0-1 · " + context.getString(R.string.black_won)
    "1/2-1/2" -> "½-½ · " + context.getString(R.string.draw)
    else -> ""
}

/// True only the very first time the board is touched (then the hint is shown).
private fun firstBoardTouch(context: Context): Boolean {
    val prefs = context.getSharedPreferences("hints", Context.MODE_PRIVATE)
    if (prefs.getBoolean("board", false)) return false
    prefs.edit().putBoolean("board", true).apply()
    return true
}
