package io.github.vrubelroman.chesspenguin.ui

import android.content.Context
import android.content.res.Configuration
import android.content.Intent
import android.view.HapticFeedbackConstants
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.indication
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.PressInteraction
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.windowInsetsPadding
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
import androidx.compose.material.icons.outlined.Memory
import androidx.compose.material.icons.rounded.ChevronLeft
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Save
import androidx.compose.material.icons.rounded.School
import androidx.compose.material.icons.rounded.SmartToy
import androidx.compose.material.icons.rounded.VerticalAlignTop
import androidx.compose.material.icons.rounded.ChevronRight
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.FirstPage
import androidx.compose.material.icons.rounded.LastPage
import androidx.compose.material.icons.rounded.Memory
import androidx.compose.material.icons.rounded.MoreVert
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.SwapVert
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.ripple
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
import androidx.compose.ui.draw.scale
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.pluralStringResource
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
import androidx.lifecycle.viewmodel.compose.viewModel
import io.github.vrubelroman.chesspenguin.data.Source
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.EngineSession
import io.github.vrubelroman.chesspenguin.data.NetState
import io.github.vrubelroman.chesspenguin.data.REVIEW_NODES
import io.github.vrubelroman.chesspenguin.data.ReviewSession
import io.github.vrubelroman.chesspenguin.data.enginePosition
import io.github.vrubelroman.chesspenguin.theme.LocalChessColors
import io.github.vrubelroman.chesspenguin.theme.Tabular
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.mobile_api.EngineLine
import uniffi.mobile_api.Edited
import uniffi.mobile_api.GameData
import uniffi.mobile_api.GameSession
import uniffi.mobile_api.MoveJudgement
import uniffi.mobile_api.MoveNode

/// An engine line shown on the board (§8.3): the game itself does not change. `best`:
/// the better line of an error of the game report (§21.1).
private data class Preview(val line: EngineLine, val step: Int, val best: Boolean = false)

/// One game: the board on top, the notation under it, the move buttons at the bottom
/// under the thumb; ‹ › page the games of the list (android-ui.md §7).
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun GameScreen(db: String, startPlace: Int, query: String, onBack: () -> Unit, onSettings: () -> Unit, onPlayGame: () -> Unit = {}) {
    val app = app()
    val context = LocalContext.current
    val prefs by app.settings.prefs.collectAsState()
    val libraryEntries by app.library.entries.collectAsState()
    val dbEntry = libraryEntries.firstOrNull { it.id == db }
    var games by remember { mutableStateOf(app.shown[db]) }
    var place by rememberSaveable { mutableIntStateOf(startPlace) }
    // The game as changed on the board (android-play.md §29), kept while the phone turns:
    // a game of someone else's file is changed for analysis and never written there.
    val held: GameEdit = viewModel()
    var game by held.data
    var edit by held.session
    val newGame = db == NEW_GAME
    // The game against Maia in hand (android-play.md §31).
    val playGame = db == PLAY_GAME
    val run = held.play.value
    // Learning the opening of this game: its own screen over the game (§32).
    held.trainer.value?.let { t ->
        TrainScreen(t, held.trainPly, held.trainFlipped, onPly = { held.trainPly = it }, onExit = {
            held.trainer.value = null
            t.close()
        })
        return
    }
    var failed by remember { mutableStateOf(false) }
    var current by rememberSaveable(place) { mutableIntStateOf(0) }
    var flipped by rememberSaveable { mutableStateOf(false) }
    var preview by remember { mutableStateOf<Preview?>(null) }
    // The game report (§21): its own engine job; the live engine rests while it runs.
    val review = remember { ReviewSession(java.io.File(context.cacheDir, "analysis").path, app.scope) }
    LaunchedEffect(db, place, held.reloads.intValue) {
        preview = null
        if (playGame) {
            if (held.key != PLAY_GAME) {
                val meta = app.maiaGames.current.value
                val pgn = app.maiaGames.pgn()
                if (meta == null || pgn == null) {
                    failed = true
                    return@LaunchedEffect
                }
                held.key = PLAY_GAME
                held.savedTo = null
                val s = GameSession.fromPgn(pgn)
                edit = s
                val d = s.data()
                game = d
                held.play.value?.close()
                held.play.value = PlayRun(meta).also { r ->
                    // A game that ended before the app was closed: its end as it was.
                    if (meta.over) r.over.value = PlayOver(d.result, if (d.result == "1/2-1/2") R.string.play_draw else if ((d.result == "1-0") == meta.userWhite) R.string.play_won else R.string.play_lost, null)
                }
                flipped = !meta.userWhite
                current = lineEnd(d)
            }
            return@LaunchedEffect
        }
        if (newGame) {
            if (held.key != NEW_GAME) {
                held.key = NEW_GAME
                edit = GameSession.newGame(java.text.SimpleDateFormat("yyyy.MM.dd", java.util.Locale.ROOT).format(java.util.Date()))
                game = edit?.data()
            }
            return@LaunchedEffect
        }
        val database = app.library.database(db).getOrNull()
        if (database == null) {
            failed = true
            return@LaunchedEffect
        }
        // After the process was restarted the list is gone: the same search finds it again.
        val list = games ?: withContext(Dispatchers.Default) { app.library.listed(db, database, query) }.also {
            games = it
            app.shown[db] = it
        }
        val index = list.getOrNull(place) ?: return@LaunchedEffect
        val key = "$db/$index"
        // The phone turned: the game as it was changed is still there.
        if (held.key != key || edit == null) {
            game = null
            edit = null
            held.savedTo = null
            val loaded = withContext(Dispatchers.Default) { runCatching { database.game(index) to database.session(index) } }
            failed = loaded.isFailure
            game = loaded.getOrNull()?.first
            edit = loaded.getOrNull()?.second
            held.notes = game?.notes ?: emptyList()
            held.key = key
        }
        if (game != null) review.open(db, database, index)
    }
    val pieces = remember { Pieces(context) }
    val snackbar = remember { SnackbarHostState() }
    UpdateMessages(snackbar)
    val scope = rememberCoroutineScope()
    val view = LocalView.current
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
    // Read only where it is shown (the strip, the arrows): the engine speaks several
    // times a second, the rest of the screen is not drawn again for it.
    val engineView = session.analysis.collectAsState()
    var visible by remember { mutableStateOf(true) }
    var sheet by rememberSaveable { mutableStateOf(false) }
    var wantEngine by rememberSaveable { mutableStateOf(false) }
    // The engine button: off in every game screen opened, on only when pressed, and
    // only until the app goes to the background (§8.3). Kept while Settings is open over
    // the game and when the phone is turned.
    var engineVisit by rememberSaveable { mutableStateOf<String?>(null) }
    val engineWanted = engineVisit == app.visit
    var expanded by rememberSaveable { mutableStateOf(false) }
    val reviewState by review.state.collectAsState()
    // "Hide" in the report: until the next game.
    var reviewHidden by remember(place) { mutableStateOf(false) }
    var reviewStart by remember { mutableStateOf(false) }
    var reviewReport by remember { mutableStateOf(false) }
    // A start that waits for the engine (downloaded or unpacked): full or not.
    var wantReview by remember { mutableStateOf<Boolean?>(null) }
    // The download sheet was opened to analyse the game (not for the engine button).
    var sheetForReview by remember { mutableStateOf<Boolean?>(null) }
    // Stopped because the app went to the background: goes on when it comes back.
    // Kept when the phone is turned (the screen is made again): the analysis goes on.
    var reviewResume by rememberSaveable(place) { mutableStateOf(false) }
    val reviewCycle = remember(place) { HashMap<Pair<Boolean, MoveJudgement>, Int>() }
    var menu by remember { mutableStateOf(false) }
    var info by remember { mutableStateOf(false) }
    @Suppress("DEPRECATION")
    val clipboard = androidx.compose.ui.platform.LocalClipboardManager.current
    val pgnCopied = stringResource(R.string.pgn_copied)
    val fenCopied = stringResource(R.string.fen_copied)
    suspend fun pgnText(): String? {
        // Moves added on the board go along (the file is as it was).
        edit?.takeIf { it.isModified() }?.let { e -> return withContext(Dispatchers.Default) { e.pgn() } }
        val index = games?.getOrNull(place) ?: return null
        val database = app.library.database(db).getOrNull() ?: return null
        return withContext(Dispatchers.Default) { database.pgn(index) }
    }
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
    // The player bars above and below the board (§7.11); the top bar names the event.
    val whiteName = tag("White").trim().takeIf { it.isNotEmpty() && it != "?" }
    val blackName = tag("Black").trim().takeIf { it.isNotEmpty() && it != "?" }
    val whiteLabel = stringResource(R.string.tag_white)
    val blackLabel = stringResource(R.string.tag_black)
    val resultText = g?.let { resultLine(context, it.result) } ?: ""
    val (items, where) = remember(g, resultText) { g?.let { layout(it, resultText) } ?: (emptyList<Item>() to emptyMap()) }
    val nodes = g?.nodes
    val node = nodes?.getOrNull(current)
    val notation = rememberLazyListState()
    val engineReady = netState == NetState.Ready
    val engineOn = engineWanted && engineReady
    val position = remember(g, current) { g?.let { enginePosition(it, current) } }
    // Only what the engine said about the position on the board.
    fun analysis() = engineView.value?.takeIf { it.position == position }?.analysis
    // Mate and stalemate need no engine: the strip says what it is (§7.3).
    val ending = when (node?.end) {
        "checkmate" -> stringResource(R.string.checkmate) + (g?.let { " · " + shortResult(it.result) }?.takeIf { !it.endsWith("*") } ?: "")
        "stalemate" -> stringResource(R.string.stalemate) + " · ½-½"
        else -> null
    }

    val reviewRunning = reviewState?.running == true
    LaunchedEffect(position, engineOn, visible, prefs.engineLines, prefs.engineSeconds, reviewRunning) {
        if (position == null || !visible) return@LaunchedEffect
        when {
            // One engine at a time on a phone: the analysis of the game comes first (§21.6).
            !engineOn || reviewRunning -> session.shutdown()
            ending != null -> session.pause()
            else -> session.show(position, prefs)
        }
    }
    // A download started from the button turns the engine on when it is done.
    LaunchedEffect(netState) {
        if (netState == NetState.Ready && wantEngine) {
            wantEngine = false
            sheet = false
            engineVisit = app.visit
            if (!files.builtIn) snackbar.showSnackbar(readyText)
        }
    }

    fun startReview(full: Boolean) {
        if (files.state.value != NetState.Ready) {
            if (files.builtIn) {
                wantReview = full
                files.prepare()
            } else {
                sheetForReview = full
                sheet = true
            }
            return
        }
        wantReview = null
        reviewHidden = false
        val p = app.settings.prefs.value
        review.start(files.executable, files.net.path, p.engineThreads, p.engineHashMb, REVIEW_NODES[p.reviewQuality], full) { s ->
            if (s.npsPerThread > 0UL) app.settings.update { it.copy(reviewNps = s.npsPerThread.toLong()) }
        }
    }
    // The engine became ready for an analysis that was asked for.
    LaunchedEffect(netState) {
        if (netState == NetState.Ready) wantReview?.let { full ->
            sheet = false
            startReview(full)
        }
    }
    // An analysis stopped by the background or a turn of the phone goes on once this
    // game's analysis is loaded again and the screen is visible.
    val reviewLoaded = reviewState != null
    LaunchedEffect(reviewLoaded, visible) {
        if (reviewLoaded && visible && reviewResume) {
            reviewResume = false
            startReview(false)
        }
    }
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, e ->
            when (e) {
                // Battery first (§21.6): the analysis stops with the screen and goes on after.
                Lifecycle.Event.ON_STOP -> if (review.state.value?.running == true) {
                    reviewResume = true
                    review.cancel()
                }
                else -> {}
            }
        }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            review.close()
        }
    }

    val rs = reviewState
    val preparingReview = wantReview != null && files.builtIn && netState is NetState.Loading
    // A game without moves has nothing to analyse.
    val hasMoves = g?.nodes?.firstOrNull()?.children?.isNotEmpty() == true
    val reviewShown = rs != null && hasMoves && !reviewHidden && (rs.hasData || rs.running || preparingReview)
    val reviewPoints = remember(rs) { rs?.points?.withIndex()?.associate { it.value.node.toInt() to it.index } ?: emptyMap() }
    val reviewMarks = remember(rs) { rs?.moves?.associate { it.node.toInt() to it.judgement } ?: emptyMap() }
    val currentMark = if (reviewShown) rs.moves.firstOrNull { it.node.toInt() == current } else null

    fun buzz(edge: Boolean) {
        if (prefs.vibration) view.performHapticFeedback(if (edge) HapticFeedbackConstants.CONFIRM else HapticFeedbackConstants.CLOCK_TICK)
    }
    // A change of the game: shown at once, the cursor where the change says.
    fun apply(e: Edited) {
        preview = null
        game = e.data.copy(notes = held.notes)
        current = e.cursor.toInt()
    }
    // The move sheet of a long press on a move (§29.6): which move.
    var moveSheet by remember { mutableStateOf<Int?>(null) }
    // «Play on from here» asked for at this move.
    var playFrom by remember { mutableStateOf<Int?>(null) }
    val deletedText = stringResource(R.string.moves_deleted)
    val undoText = stringResource(R.string.undo_action)
    fun undo() {
        edit?.undo()?.let { apply(it) }
    }

    // ----- a game against Maia (§31) -----
    val runOver = run?.over?.value
    val runOffer = run?.offer?.value == true
    val runThinking = run?.thinking?.value == true
    val runFailure = run?.failure?.value
    val gameOn = run != null && runOver == null
    fun persist() {
        if (run != null) edit?.let { app.maiaGames.save(it.pgn()) }
    }
    fun finishGame(over: PlayOver) {
        val r = run ?: return
        val e = edit ?: return
        r.generation++
        r.thinking.value = false
        r.offer.value = false
        e.setData(listOf(uniffi.mobile_api.Tag("Termination", "Normal")), over.result)
        game = e.data().copy(notes = held.notes)
        r.over.value = over
        app.maiaGames.finish(e.pgn())
    }
    // After a move of either side: kept, and the end of the game if it came.
    fun afterMove(e: Edited) {
        val r = run ?: return
        persist()
        edit?.ending(e.cursor)?.let { finishGame(overBy(it, r.meta.userWhite)) }
    }
    // Maia's move when it is her turn: computed in the background, shown after her pause.
    LaunchedEffect(run, g, runOver, runOffer, runFailure) {
        val r = run ?: return@LaunchedEffect
        val gg = g ?: return@LaunchedEffect
        val e = edit ?: return@LaunchedEffect
        if (runOver != null || runOffer || runFailure != null) return@LaunchedEffect
        val end = lineEnd(gg)
        if (whiteToMove(gg.nodes[end].fen) == r.meta.userWhite) return@LaunchedEffect
        val gen = ++r.generation
        r.thinking.value = true
        try {
            val player = r.player ?: withContext(Dispatchers.IO) { runCatching { uniffi.mobile_api.MaiaPlayer.load(app.maiaFiles.download.file.path) } }
                .getOrElse { ex ->
                    r.failure.value = ex.message ?: ""
                    return@LaunchedEffect
                }.also { r.player = it }
            // Her resignation is judged by Stockfish, when it is there.
            if (r.judge == null && files.state.value == NetState.Ready) {
                r.judge = runCatching { uniffi.mobile_api.ResignJudge.start(files.executable, files.net.path) }.getOrNull()
            }
            r.judge?.ask(e, end.toUInt())
            val started = System.currentTimeMillis()
            val only = e.legalMoves(end.toUInt()).size == 1
            val uci = withContext(Dispatchers.Default) {
                player.choose(e, end.toUInt(), r.meta.strength.toUInt(), null, kotlin.random.Random.nextFloat())
            } ?: return@LaunchedEffect
            val wait = uniffi.mobile_api.maiaPauseMs(depth(gg, end).toUInt(), only).toLong() - (System.currentTimeMillis() - started)
            if (wait > 0) delay(wait)
            if (gen != r.generation) return@LaunchedEffect
            r.judge?.hopeless()?.let { if (it) r.hopeless++ else r.hopeless = 0u }
            if (!r.playOn && r.hopeless >= uniffi.mobile_api.hopelessTurns()) {
                r.offer.value = true
                return@LaunchedEffect
            }
            runCatching { e.play(end.toUInt(), uci) }.getOrNull()?.let { ed ->
                apply(ed)
                afterMove(ed)
            }
        } finally {
            if (gen == r.generation) r.thinking.value = false
        }
    }
    var resigning by remember { mutableStateOf(false) }
    // The result card closed with ×: until the next game.
    var resultClosed by remember(run) { mutableStateOf(false) }
    val eventText = stringResource(R.string.play_event)
    val noteText = stringResource(R.string.play_note_from, (run?.meta?.strength ?: 0).toString())
    // The same strength again, colours swapped; from a position: the same one and colour.
    fun rematch() {
        val r = run ?: return
        val e = edit ?: return
        val gg = g ?: return
        val m = r.meta
        val base = if (m.floor > 0) {
            var n = 0
            repeat(m.floor) { n = gg.nodes[n].children.firstOrNull()?.toInt() ?: n }
            runCatching { e.playFrom(n.toUInt()) }.getOrNull() ?: return
        } else {
            GameSession.newGame("")
        }
        beginMaiaGame(app, base, m.strength, if (m.floor > 0) m.userWhite else !m.userWhite, m.name, m.floor, eventText, noteText)
        base.close()
        held.key = null
        held.reloads.intValue++
    }
    fun takeBack() {
        val r = run ?: return
        val gg = g ?: return
        val e = edit ?: return
        val end = lineEnd(gg)
        // While Maia thinks the last move is the user's; else hers, and the user's before.
        val from = if (r.thinking.value) end else gg.nodes[end].parent?.toInt() ?: return
        if (depth(gg, from) <= r.meta.floor) return
        r.generation++
        r.thinking.value = false
        r.judge?.forget()
        r.hopeless = 0u
        runCatching { e.deleteFrom(from.toUInt()) }.getOrNull()?.let {
            apply(it)
            persist()
        }
    }

    // Saving (android-play.md §30.4): an own database's game in its place; a new game or
    // a copy of someone else's into an own database chosen once, then in its place there.
    val ownGame = dbEntry?.source == Source.Own && !newGame
    var saves by remember { mutableIntStateOf(0) }
    @Suppress("UNUSED_EXPRESSION") saves
    val modified = g != null && edit?.isModified() == true
    var saving by remember { mutableStateOf(false) }
    // The game's data form; `firstSave`: a new game's first save asks for it first.
    var editingData by remember { mutableStateOf(false) }
    var firstSave by remember { mutableStateOf(false) }
    var leaving by remember { mutableStateOf(false) }
    var afterSave by remember { mutableStateOf<(() -> Unit)?>(null) }
    val ownEntries = libraryEntries.filter { it.source == Source.Own }
    val savedText = stringResource(R.string.saved_to)
    val saveFailedText = stringResource(R.string.save_failed)
    val lastOwn = remember { context.getSharedPreferences("own", Context.MODE_PRIVATE) }
    fun saveInto(id: String, index: UInt?) {
        val e = edit ?: return
        val then = afterSave
        afterSave = null
        scope.launch {
            val pgn = withContext(Dispatchers.Default) { e.pgn() }
            val done = runCatching { app.library.saveOwn(id, index, pgn) }
            snackbar.currentSnackbarData?.dismiss()
            done.onSuccess { n ->
                e.markSaved()
                saves++
                if (!ownGame) held.savedTo = id to n
                lastOwn.edit().putString("last", id).apply()
                val name = app.library.entry(id)?.name ?: ""  // own databases: their own names
                then?.invoke() ?: snackbar.showSnackbar(String.format(savedText, name, (n + 1u).toString()))
            }.onFailure { snackbar.showSnackbar(saveFailedText) }
        }
    }
    fun save(then: (() -> Unit)? = null) {
        afterSave = then
        val to = held.savedTo
        when {
            ownGame -> games?.getOrNull(place)?.let { saveInto(db, it) }
            to != null -> saveInto(to.first, to.second)
            // A new game's names and result first, as the desktop asks when adding.
            newGame && !playGame -> {
                firstSave = true
                editingData = true
            }
            else -> saving = true
        }
    }
    // Leaving with changes asks only where they are the user's to keep (an own game, a
    // new one); moves in someone else's game are for trying out (owner's decision).
    // A game against Maia: put aside while it goes on (no question); after its end like
    // a new game, and gone from hand once left.
    val asksOnLeave = modified && (ownGame || newGame || (playGame && runOver != null))
    val exit: () -> Unit = {
        if (playGame && held.play.value?.over?.value != null) app.maiaGames.clear()
        onBack()
    }
    // What leaving's question leads to (back, or a rematch).
    var leaveTo by remember { mutableStateOf<(() -> Unit)?>(null) }
    fun leave() {
        if (asksOnLeave) {
            leaveTo = exit
            leaving = true
        } else {
            exit()
        }
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
        // Read from the states now: the button may hold this function as it was when the
        // screen was first drawn.
        val ready = files.state.value == NetState.Ready
        when {
            ready && engineVisit == app.visit -> engineVisit = null
            ready -> engineVisit = app.visit
            files.builtIn -> {
                wantEngine = true
                files.prepare()
            }
            else -> {
                sheetForReview = null
                sheet = true
            }
        }
    }
    BackHandler(preview != null) { preview = null }
    BackHandler(preview == null && asksOnLeave) { leaving = true }

    val landscape = LocalConfiguration.current.orientation == Configuration.ORIENTATION_LANDSCAPE
    val controls: @Composable (Modifier) -> Unit = { modifier ->
        if (gameOn && run != null && g != null) {
            val end = lineEnd(g)
            val endDepth = depth(g, end)
            val floor = run.meta.floor
            PlayControls(
                // While Maia thinks the last move is the user's; else hers and the user's before.
                canTakeBack = if (runThinking) endDepth > floor else endDepth - 1 > floor,
                started = endDepth - floor >= 2,
                canBack = canBack,
                canForward = canForward,
                onTakeBack = ::takeBack,
                onResign = { resigning = true },
                onAbort = {
                    app.maiaGames.clear()
                    onBack()
                },
                onFlip = { flipped = !flipped },
                onBack = ::back,
                onForward = ::forward,
            )
        } else Column(modifier) {
            val p = preview
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
                    BarButton(Icons.Rounded.PlayArrow, stringResource(R.string.move_back), canBack, Modifier.weight(1.5f), 32, repeat = true, mirrored = true) { back() }
                    BarButton(Icons.Rounded.PlayArrow, stringResource(R.string.move_forward), canForward, Modifier.weight(1.5f), 32, repeat = true) { forward() }
                    BarButton(Icons.Rounded.LastPage, stringResource(R.string.to_end), canForward, Modifier.weight(1f)) { end() }
                }
            }
        }
    }
    val board: @Composable (Modifier) -> Unit = { boardModifier ->
        val dark = LocalChessColors.current.dark
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
                // After an error of the report: how the opponent punishes it (§21.1).
                if (!engineOn) currentMark?.let { m ->
                    val f = m.replyFrom
                    val t = m.replyTo
                    if (f != null && t != null) add(BoardArrow(f, t, judgementColor(m.judgement, dark).copy(alpha = 0.8f), 0.9f))
                }
                if (engineOn && prefs.bestArrow) {
                    analysis()?.lines?.take(2)?.forEachIndexed { i, l ->
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
            theme = prefs.board,
            set = prefs.pieces,
            description = description,
            arrows = arrows,
            marks = marks,
            check = check,
            animated = prefs.animation,
            // A touch during a preview of an engine line closes it; no move is made (§29.5).
            modifier = if (p != null) boardModifier.pointerInput(Unit) { detectTapGestures { preview = null } } else boardModifier,
            input = edit?.takeIf {
                // In a game: only the user's side, in their turn, at the game's last move.
                p == null && g != null && (!gameOn || (current == lineEnd(g) && !runThinking && !runOffer && whiteToMove(g.nodes[current].fen) == run!!.meta.userWhite))
            }?.let { e ->
                val legal = remember(g, current) { e.legalMoves(current.toUInt()) }
                BoardInput(legal, onMove = { uci ->
                    runCatching { e.play(current.toUInt(), uci) }.getOrNull()?.let {
                        apply(it)
                        if (gameOn) afterMove(it)
                    }
                })
            },
        )
    }
    val side: @Composable (Modifier) -> Unit = { sideModifier ->
        Column(sideModifier.fillMaxWidth()) {
            val p = preview
            if (engineOn) {
                Live {
                    EngineStrip(
                        ending = ending,
                        status = status,
                        analysis = analysis(),
                        flipped = flipped,
                        lines = prefs.engineLines,
                        expanded = expanded,
                        onExpand = { expanded = !expanded },
                        onLine = { l -> if (l.steps.isNotEmpty()) preview = Preview(l, 0) },
                        onDeeper = { session.deeper() },
                        onRestart = {
                            session.shutdown()
                            position?.let { session.show(it, prefs) }
                        },
                        notation = line,
                        waiting = reviewRunning,
                    )
                }
            } else if (wantEngine && files.builtIn && netState is NetState.Loading) {
                Row(Modifier.fillMaxWidth().height(48.dp).padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    Text(stringResource(R.string.preparing_engine), Modifier.padding(start = 12.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            if (reviewShown) {
                val cursor = g?.let { pointIndex(reviewPoints, it, current) }
                ReviewPanel(
                    state = rs,
                    cursor = cursor,
                    // TalkBack: "6…Nf6, evaluation +1.2" (the start position has no move).
                    cursorLabel = cursor?.takeIf { it > 0 }?.let { rs.points[it].node.toInt() }?.let { id ->
                        nodes?.getOrNull(id)?.let { n -> (if (n.white) "${n.moveNumber}." else "${n.moveNumber}…") + n.san }
                    },
                    // A phone on its side has little height: with the engine strip on, only the row (§21.3).
                    showGraph = !(landscape && engineOn),
                    mark = currentMark,
                    preparing = preparingReview,
                    notation = line,
                    onIndex = { i -> rs.points.getOrNull(i)?.let { go(it.node.toInt()) } },
                    onBest = { m -> m.best?.takeIf { it.steps.isNotEmpty() }?.let { preview = Preview(it, 0, best = true) } },
                    onReport = { reviewReport = true },
                    onStop = { review.cancel() },
                    onResume = { startReview(false) },
                )
            }
            if (p != null) PreviewBar(p.step + 1, p.line.steps.size, onClose = { preview = null }, best = p.best)
            if (run != null) {
                when {
                    runOffer -> OfferCard(
                        onAccept = { finishGame(PlayOver(if (run.meta.userWhite) "1-0" else "0-1", R.string.play_won, R.string.end_maia_resigned)) },
                        onPlayOn = {
                            run.playOn = true
                            run.offer.value = false
                        },
                    )
                    runFailure != null -> Text(
                        stringResource(R.string.play_maia_failed, runFailure),
                        Modifier.padding(16.dp),
                        color = MaterialTheme.colorScheme.error,
                    )
                    runOver != null && !resultClosed -> ResultCard(
                        runOver,
                        saved = held.savedTo != null && !modified,
                        onAnalyse = {
                            val e = edit
                            if (e != null) scope.launch {
                                review.openSession("${held.key}/${System.nanoTime()}", e)
                                reviewHidden = false
                                reviewStart = true
                            }
                        },
                        onRematch = {
                            val go = { rematch() }
                            if (modified) {
                                leaveTo = go
                                leaving = true
                            } else {
                                go()
                            }
                        },
                        onSave = { save() },
                        onClose = { resultClosed = true },
                    )
                }
            }
            Box(Modifier.weight(1f).fillMaxWidth()) {
                when {
                    g != null -> Notation(
                        g, items, where, current, notation, san, onMove = ::go, modifier = Modifier.fillMaxSize(),
                        marks = if (reviewShown) reviewMarks else emptyMap(),
                        onLongMove = edit?.let {
                            { id ->
                                go(id)
                                buzz(true)
                                moveSheet = id
                            }
                        },
                    )
                    failed -> Text(
                        stringResource(R.string.game_failed),
                        color = MaterialTheme.colorScheme.error,
                        modifier = Modifier.align(Alignment.Center),
                    )
                    else -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                }
            }
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                navigationIcon = {
                    IconButton(onClick = ::leave) { Icon(Icons.AutoMirrored.Rounded.ArrowBack, stringResource(R.string.back)) }
                },
                title = {
                    // The event and year (else the site); the game's place and round. A tap
                    // shows the game's details.
                    // No event and no site: the database's name.
                    val event = if (playGame && run != null) {
                        stringResource(R.string.play_with_maia) + " " + run.meta.strength
                    } else if (newGame && held.savedTo == null) {
                        stringResource(R.string.new_game)
                    } else {
                        joinKnown(tag("Event"), year(tag("Date"))).ifEmpty { joinKnown(tag("Site")) }
                            .ifEmpty { dbEntry?.let { displayName(it) } ?: "" }
                    }
                    val round = tag("Round").trim().takeIf { it.isNotEmpty() && it != "?" && it != "-" }
                    val placeLine = if (playGame && run != null) {
                        stringResource(R.string.play_adjourned_sub, stringResource(if (run.meta.userWhite) R.string.play_as_white else R.string.play_as_black), stringResource(R.string.play_untimed))
                    } else joinKnown(
                        if (total > 0) stringResource(R.string.game_of, place + 1, total) else "",
                        round?.let { stringResource(R.string.tag_round).lowercase() + " " + it } ?: "",
                    )
                    Column(
                        Modifier.fillMaxWidth().clickable(
                            enabled = g != null,
                            onClickLabel = stringResource(R.string.game_info),
                            role = Role.Button,
                        ) { info = true },
                    ) {
                        Text(
                            event.ifEmpty { placeLine },
                            style = MaterialTheme.typography.titleMedium,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        if (event.isNotEmpty() && placeLine.isNotEmpty()) {
                            Text(
                                placeLine,
                                style = MaterialTheme.typography.bodyMedium.merge(Tabular),
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                },
                actions = {
                    // Changes not saved: "Save" in place of ‹ › (they would need a decision).
                    if (modified && !gameOn) {
                        val where = when {
                            ownGame -> dbEntry?.let { stringResource(R.string.save_in, displayName(it)) }
                            else -> held.savedTo?.first?.let { app.library.entry(it) }?.let { stringResource(R.string.save_in, displayName(it)) }
                        } ?: stringResource(R.string.save_to_mine)
                        IconButton(onClick = { save() }) { Icon(Icons.Rounded.Save, where) }
                    } else if (!newGame && !playGame) {
                        IconButton(onClick = { place -= 1 }, enabled = place > 0) {
                            Icon(Icons.Rounded.ChevronLeft, stringResource(R.string.previous_game))
                        }
                        IconButton(onClick = { place += 1 }, enabled = place + 1 < total) {
                            Icon(Icons.Rounded.ChevronRight, stringResource(R.string.next_game))
                        }
                    }
                    Box {
                        IconButton(onClick = { menu = true }) { Icon(Icons.Rounded.MoreVert, stringResource(R.string.menu)) }
                        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                            // First, and only when there is something to take back (§29.6).
                            if (g != null && edit?.canUndo() == true) {
                                DropdownMenuItem(text = { Text(stringResource(R.string.undo)) }, onClick = {
                                    menu = false
                                    undo()
                                })
                            }
                            DropdownMenuItem(
                                text = { Text(stringResource(if (rs?.hasData == true) R.string.review_report_menu else R.string.review_menu)) },
                                // During the first analysis the panel already shows its progress (§21.1).
                                enabled = hasMoves && !gameOn && ((newGame || playGame) || (rs != null && (rs.hasData || !rs.running))),
                                onClick = {
                                    menu = false
                                    reviewHidden = false
                                    val e = edit
                                    if ((newGame || playGame) && e != null && rs == null) {
                                        scope.launch {
                                            review.openSession("${held.key}/${System.nanoTime()}", e)
                                            reviewStart = true
                                        }
                                        return@DropdownMenuItem
                                    }
                                    when {
                                        rs?.hasData == true -> reviewReport = true
                                        rs?.running == true -> {}
                                        else -> reviewStart = true
                                    }
                                },
                            )
                            DropdownMenuItem(text = { Text(stringResource(R.string.game_info)) }, enabled = g != null, onClick = {
                                menu = false
                                info = true
                            })
                            DropdownMenuItem(text = { Text(stringResource(R.string.copy_pgn)) }, enabled = g != null, onClick = {
                                menu = false
                                scope.launch {
                                    pgnText()?.let {
                                        clipboard.setText(AnnotatedString(it))
                                        snackbar.showSnackbar(pgnCopied)
                                    }
                                }
                            })
                            DropdownMenuItem(text = { Text(stringResource(R.string.copy_fen)) }, enabled = node != null, onClick = {
                                menu = false
                                (preview?.let { it.line.steps[it.step].fen } ?: node?.fen)?.let {
                                    clipboard.setText(AnnotatedString(it))
                                    scope.launch { snackbar.showSnackbar(fenCopied) }
                                }
                            })
                            DropdownMenuItem(text = { Text(stringResource(R.string.share_game)) }, enabled = g != null, onClick = {
                                menu = false
                                scope.launch {
                                    pgnText()?.let { text ->
                                        val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)
                                        context.startActivity(Intent.createChooser(send, null))
                                    }
                                }
                            })
                            DropdownMenuItem(text = { Text(stringResource(R.string.settings)) }, onClick = {
                                menu = false
                                onSettings()
                            })
                        }
                    }
                },
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
        // In the bottom bar, so that snackbars stand above the move buttons; beside the
        // board when the phone lies on its side.
        bottomBar = { if (!landscape) controls(Modifier) },
    ) { padding ->
        val inner = Modifier.fillMaxSize().padding(top = padding.calculateTopPadding(), bottom = padding.calculateBottomPadding())
        // Black above the board, White below (swapped with the board, §7.11). A study with
        // a title for White and no Black keeps both bars, so the board does not jump.
        val study = whiteName != null && blackName == null
        val result = g?.result ?: ""
        // In a game: «thinking…» on Maia's bar while she computes (§31.3).
        val thinkingText = stringResource(R.string.play_thinking)
        val maiaThinks = { white: Boolean -> if (gameOn && runThinking && run?.meta?.userWhite != white) thinkingText else null }
        val whiteBar: @Composable () -> Unit = {
            PlayerBar(whiteName ?: whiteLabel, elo(tag("WhiteElo")), true, maiaThinks(true) ?: sideResult(result, true), known = whiteName != null, abbreviate = !study)
        }
        val blackBar: @Composable () -> Unit = {
            PlayerBar(blackName ?: blackLabel, elo(tag("BlackElo")), false, maiaThinks(false) ?: sideResult(result, false), known = blackName != null)
        }
        val topBar = if (flipped) whiteBar else blackBar
        val bottomBar = if (flipped) blackBar else whiteBar
        if (landscape) {
            // The navigation buttons stand on a side of the screen: nothing goes under them.
            // The bars go beside the board, so it keeps the full height.
            Row(inner.windowInsetsPadding(WindowInsets.navigationBars.only(WindowInsetsSides.Horizontal))) {
                board(Modifier.fillMaxHeight())
                Column(Modifier.weight(1f).fillMaxHeight()) {
                    topBar()
                    side(Modifier.weight(1f))
                    bottomBar()
                    controls(Modifier)
                }
            }
        } else {
            Column(inner) {
                topBar()
                board(Modifier.fillMaxWidth())
                bottomBar()
                side(Modifier.weight(1f))
            }
        }
    }
    if (info && g != null) {
        AlertDialog(
            onDismissRequest = { info = false },
            title = { Text(stringResource(R.string.game_info)) },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    g.tags.filter { it.value.isNotBlank() && it.value != "?" && !it.value.startsWith("????") }.forEach { t ->
                        Row(Modifier.padding(vertical = 4.dp)) {
                            Text(tagLabel(t.name), Modifier.width(112.dp), color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)
                            Text(if (t.name == "Date" || t.name == "EventDate") pgnDate(t.value) else t.value, style = MaterialTheme.typography.bodyMedium)
                        }
                    }
                    if (g.notes.isNotEmpty()) {
                        Text(stringResource(R.string.notes), Modifier.padding(top = 12.dp), style = MaterialTheme.typography.labelLarge)
                        g.notes.forEach { Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                    }
                }
            },
            confirmButton = { TextButton(onClick = { info = false }) { Text(stringResource(R.string.close)) } },
            // The user's own game, or a new one: its data can be changed (§30.5).
            dismissButton = if (ownGame || newGame || held.savedTo != null) {
                {
                    TextButton(onClick = {
                        info = false
                        editingData = true
                    }) { Text(stringResource(R.string.edit)) }
                }
            } else {
                null
            },
        )
    }
    if (editingData && g != null) {
        GameDataForm(
            g,
            doneLabel = stringResource(if (firstSave) R.string.next else R.string.done),
            onDone = { tags, result ->
                editingData = false
                edit?.let { e ->
                    e.setData(tags, result)
                    game = e.data().copy(notes = held.notes)
                    saves++
                }
                if (firstSave) {
                    firstSave = false
                    saving = true
                }
            },
            onDismiss = {
                editingData = false
                firstSave = false
                afterSave = null
            },
        )
    }
    if (saving) {
        SaveSheet(
            own = ownEntries,
            last = lastOwn.getString("last", null),
            onSave = { e ->
                saving = false
                saveInto(e.id, null)
            },
            onCreate = { name ->
                saving = false
                scope.launch { runCatching { app.library.createOwn(name) }.onSuccess { saveInto(it.id, null) } }
            },
            onDismiss = {
                saving = false
                afterSave = null
            },
        )
    }
    if (leaving) {
        AlertDialog(
            onDismissRequest = { leaving = false },
            title = {
                Text(
                    if (ownGame) stringResource(R.string.save_changes_in, dbEntry?.let { displayName(it) } ?: "")
                    else stringResource(R.string.save_game_question),
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    leaving = false
                    save(then = leaveTo ?: exit)
                }) { Text(stringResource(if (ownGame || held.savedTo != null) R.string.save else R.string.save_to_db_ellipsis)) }
            },
            dismissButton = {
                Row {
                    TextButton(onClick = {
                        leaving = false
                        (leaveTo ?: exit)()
                    }) { Text(stringResource(R.string.dont_save)) }
                    TextButton(onClick = { leaving = false }) { Text(stringResource(R.string.cancel)) }
                }
            },
        )
    }
    // «Play on from here» (§31.6): the moves up to the move, then a game against Maia.
    val fromMove = playFrom
    if (fromMove != null && g != null && edit != null) {
        val n = g.nodes.getOrNull(fromMove)
        if (n == null) {
            playFrom = null
        } else {
            val moveLabel = (if (n.white) "${n.moveNumber}." else "${n.moveNumber}…") + san(n.san).text
            val origin = listOfNotNull(whiteName, blackName).joinToString(" — ").takeIf { it.isNotEmpty() }
            PlaySheet(
                PlayFrom(n.fen, listOfNotNull(stringResource(R.string.play_from_label, moveLabel), origin).joinToString("\n"), whiteToMove(n.fen)),
                onPlay = { strength, white, name ->
                    playFrom = null
                    val s = runCatching { edit?.playFrom(fromMove.toUInt()) }.getOrNull() ?: return@PlaySheet
                    beginMaiaGame(app, s, strength, white, name, depth(g, fromMove), eventText, context.getString(R.string.play_note_from, strength.toString()))
                    s.close()
                    onPlayGame()
                },
                onDismiss = { playFrom = null },
            )
        }
    }
    if (resigning) {
        AlertDialog(
            onDismissRequest = { resigning = false },
            title = { Text(stringResource(R.string.play_resign_q)) },
            confirmButton = {
                TextButton(onClick = {
                    resigning = false
                    run?.let { finishGame(PlayOver(if (it.meta.userWhite) "0-1" else "1-0", R.string.play_you_resigned, null)) }
                }) { Text(stringResource(R.string.play_resign).removePrefix("⚑ "), color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { resigning = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
    val sheetMove = moveSheet
    if (sheetMove != null && g != null && edit != null) {
        val n = g.nodes.getOrNull(sheetMove)
        if (n == null || n.parent == null) {
            moveSheet = null
        } else {
            MoveSheet(
                label = AnnotatedString(if (n.white) "${n.moveNumber}." else "${n.moveNumber}…") + san(n.san),
                inVariation = g.nodes[n.parent!!.toInt()].children.firstOrNull()?.toInt() != sheetMove,
                runs = remember(sheetMove) { edit?.trainingRuns(sheetMove.toUInt())?.toInt() },
                canTrain = !n.nullMove,
                // Not at mate or stalemate, nor after a null move; not inside a game being played.
                canPlay = !n.nullMove && n.end.isEmpty() && !gameOn,
                onPlay = {
                    moveSheet = null
                    playFrom = sheetMove
                },
                onTrain = {
                    moveSheet = null
                    edit?.trainer(sheetMove.toUInt())?.let { t ->
                        preview = null
                        held.trainPly = 0
                        held.trainFlipped = flipped
                        held.trainer.value = t
                    }
                },
                onMakeMain = {
                    moveSheet = null
                    runCatching { edit?.makeMain(sheetMove.toUInt()) }.getOrNull()?.let { apply(it) }
                },
                onDelete = {
                    moveSheet = null
                    runCatching { edit?.deleteFrom(sheetMove.toUInt()) }.getOrNull()?.let { e ->
                        apply(e)
                        scope.launch {
                            if (snackbar.showSnackbar(deletedText, undoText, duration = SnackbarDuration.Short) == SnackbarResult.ActionPerformed) undo()
                        }
                    }
                },
                onDismiss = { moveSheet = null },
            )
        }
    }
    if (sheet) {
        EngineSheet(
            state = netState,
            metered = remember(sheet) { !files.builtIn && files.metered() },
            onDownload = {
                // Asked for by the analysis of the game: it starts when the engine is ready,
                // the engine button stays as it is.
                val forReview = sheetForReview
                if (forReview != null) wantReview = forReview else wantEngine = true
                files.prepare()
            },
            onCancel = {
                files.cancel()
                wantEngine = false
                wantReview = null
                sheetForReview = null
                sheet = false
                scope.launch { snackbar.showSnackbar(cancelledText) }
            },
            onDismiss = {
                sheet = false
                sheetForReview = null
            },
        )
    }
    if (reviewStart) {
        ReviewStartSheet(
            state = rs,
            quality = prefs.reviewQuality,
            threads = prefs.engineThreads,
            npsPerThread = prefs.reviewNps,
            onQuality = { q -> app.settings.update { it.copy(reviewQuality = q) } },
            onStart = { full ->
                reviewStart = false
                startReview(full)
            },
            onDismiss = { reviewStart = false },
        )
    }
    if (reviewReport && rs != null) {
        ReviewSheet(
            state = rs,
            white = whiteName ?: whiteLabel,
            black = blackName ?: blackLabel,
            onError = { w, j ->
                reviewReport = false
                val hits = rs.moves.filter { it.white == w && it.judgement == j }
                if (hits.isNotEmpty()) {
                    val k = ((reviewCycle[w to j] ?: -1) + 1) % hits.size
                    reviewCycle[w to j] = k
                    go(hits[k].node.toInt())
                }
            },
            onAgain = {
                reviewReport = false
                reviewStart = true
            },
            onHide = {
                reviewReport = false
                reviewHidden = true
            },
            onDismiss = { reviewReport = false },
        )
    }
}

private const val START = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"

/// The engine switch of the bottom bar, with its label (§7.4): filled and underlined
/// when on; with a download mark while the light build has no engine yet.
@Composable
private fun EngineButton(on: Boolean, needsDownload: Boolean, loading: NetState.Loading?, modifier: Modifier, onClick: () -> Unit) {
    val label = stringResource(R.string.engine)
    val state = stringResource(
        when {
            needsDownload -> R.string.engine_state_missing
            on -> R.string.engine_state_on
            else -> R.string.engine_state_off
        },
    )
    val action by rememberUpdatedState(onClick)
    val color = if (on) MaterialTheme.colorScheme.primary else LocalContentColor.current
    Column(
        modifier
            .fillMaxHeight()
            .semantics {
                contentDescription = label
                role = Role.Switch
                stateDescription = state
                onClick { action(); true }
            }
            .clickable(onClick = action),
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
    mirrored: Boolean = false,
    onClick: () -> Unit,
) {
    val action by rememberUpdatedState(onClick)
    val on by rememberUpdatedState(enabled)
    val interaction = remember { MutableInteractionSource() }
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
            .indication(interaction, ripple(bounded = false, radius = 32.dp))
            .pointerInput(repeat) {
                detectTapGestures(onPress = { at ->
                    if (!on) return@detectTapGestures
                    val press = PressInteraction.Press(at)
                    interaction.emit(press)
                    action()
                    coroutineScope {
                        val job = if (repeat) {
                            launch {
                                delay(400)
                                while (on) {
                                    action()
                                    delay(120)
                                }
                            }
                        } else {
                            null
                        }
                        tryAwaitRelease()
                        job?.cancel()
                    }
                    interaction.emit(PressInteraction.Release(press))
                })
            },
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, null, tint = color, modifier = Modifier.size(iconSize.dp).then(if (mirrored) Modifier.scale(-1f, 1f) else Modifier))
    }
}

/// The names of the usual PGN tags in the language of the interface; others as they are.
@Composable
private fun tagLabel(name: String): String = when (name) {
    "Event" -> stringResource(R.string.tag_event)
    "Site" -> stringResource(R.string.tag_site)
    "Date" -> stringResource(R.string.tag_date)
    "Round" -> stringResource(R.string.tag_round)
    "White" -> stringResource(R.string.tag_white)
    "Black" -> stringResource(R.string.tag_black)
    "Result" -> stringResource(R.string.tag_result)
    "WhiteElo" -> stringResource(R.string.tag_white_elo)
    "BlackElo" -> stringResource(R.string.tag_black_elo)
    "ECO" -> "ECO"
    "Annotator" -> stringResource(R.string.tag_annotator)
    "PlyCount" -> stringResource(R.string.tag_plies)
    else -> name
}

/// "1858.??.??" → "1858", "1927.09.16" → "16.09.1927".
private fun pgnDate(date: String): String {
    val parts = date.split('.')
    if (parts.size != 3) return date
    val (y, m, d) = parts
    return when {
        m.all { it.isDigit() } && d.all { it.isDigit() } -> "$d.$m.$y"
        m.all { it.isDigit() } -> "$m.$y"
        else -> y
    }
}

/// "Carlsen, Magnus" → "Carlsen"; "Magnus Carlsen" stays as it is.
/// A player above or below the board (§7.11): the side's disc, the name (full while it
/// fits), the rating, the share of the result. A missing name shows the side, greyed.
@Composable
private fun PlayerBar(name: String, elo: String, white: Boolean, result: String?, known: Boolean, abbreviate: Boolean = true) {
    PlayerLine(
        name = name,
        elo = elo,
        result = result,
        // A missing name shows the side greyed and in the regular weight (§7.11).
        style = MaterialTheme.typography.titleSmall.let { if (known) it else it.copy(fontWeight = androidx.compose.ui.text.font.FontWeight.Normal) },
        modifier = Modifier.fillMaxWidth().heightIn(min = 36.dp).padding(horizontal = 16.dp),
        disc = white,
        nameColor = if (known) androidx.compose.ui.graphics.Color.Unspecified else MaterialTheme.colorScheme.onSurfaceVariant,
        abbreviate = abbreviate,
    )
}

/// "1-0 · White won" for the end of the notation.
private fun resultLine(context: Context, result: String): String = when (result) {
    "1-0" -> "1-0 · " + context.getString(R.string.white_won)
    "0-1" -> "0-1 · " + context.getString(R.string.black_won)
    "1/2-1/2" -> "½-½ · " + context.getString(R.string.draw)
    else -> ""
}

/// True only the very first time the board is touched (then the hint is shown).
/// A scope of its own: what is read in it is drawn again without its surroundings.
@Composable
private fun Live(content: @Composable () -> Unit) = content()

/// What a long press on a move offers (android-play.md §29.6): make its line the main
/// one (in a variation), remove it with what follows.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun MoveSheet(
    label: AnnotatedString,
    inVariation: Boolean,
    runs: Int?,
    canTrain: Boolean,
    onMakeMain: () -> Unit,
    onDelete: () -> Unit,
    onTrain: () -> Unit,
    canPlay: Boolean,
    onPlay: () -> Unit,
    onDismiss: () -> Unit,
) {
    androidx.compose.material3.ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.fillMaxWidth().padding(bottom = 24.dp)) {
            Text(label, style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp))
            if (inVariation) SheetAction(Icons.Rounded.VerticalAlignTop, stringResource(R.string.make_main), onClick = onMakeMain)
            SheetAction(Icons.Rounded.Close, stringResource(R.string.delete_move), onClick = onDelete)
            HorizontalDivider(Modifier.padding(vertical = 4.dp), color = MaterialTheme.colorScheme.outlineVariant)
            SheetAction(Icons.Rounded.SmartToy, stringResource(R.string.play_from_maia), enabled = canPlay, onClick = onPlay)
            SheetAction(
                Icons.Rounded.School,
                stringResource(R.string.train_learn),
                trailing = runs?.takeIf { it > 0 }?.let { pluralStringResource(R.plurals.train_runs, it, it) },
                enabled = canTrain,
                onClick = onTrain,
            )
        }
    }
}

@Composable
private fun SheetAction(icon: ImageVector, text: String, trailing: String? = null, enabled: Boolean = true, onClick: () -> Unit) {
    val alpha = if (enabled) 1f else 0.38f
    Row(
        Modifier.fillMaxWidth().height(56.dp).clickable(enabled = enabled, onClick = onClick).padding(horizontal = 24.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(icon, null, tint = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = alpha))
        Text(text, Modifier.weight(1f).padding(start = 16.dp), style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurface.copy(alpha = alpha))
        trailing?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
    }
}
