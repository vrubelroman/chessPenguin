package io.github.vrubelroman.chesspenguin.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.DownloadView
import io.github.vrubelroman.chesspenguin.data.Source
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.mobile_api.AccountLookup
import uniffi.mobile_api.OnlineAccount
import uniffi.mobile_api.OnlineFailure
import uniffi.mobile_api.OnlineProblem
import uniffi.mobile_api.Site
import uniffi.mobile_api.findAccount
import uniffi.mobile_api.onlineDatabase
import uniffi.mobile_api.onlineTargets
import uniffi.mobile_api.validNickname
import java.io.File
import kotlin.math.ceil

/// Lichess gives about this many games a second to an anonymous export.
private const val LICHESS_GAMES_PER_SECOND = 20.0

fun siteName(s: Site): String = when (s) {
    Site.LICHESS -> "Lichess"
    Site.CHESS_COM -> "Chess.com"
}

/// "1 234 games" in the language of the interface.
@Composable
fun gamesText(n: Long): String {
    val k = n.coerceAtMost(Int.MAX_VALUE.toLong()).toInt()
    return pluralStringResource(R.plurals.games_count, k, groupDigits(k))
}

/// Seconds Lichess needs for `games` games.
private fun lichessSeconds(games: Long): Double = games / LICHESS_GAMES_PER_SECOND

/// What went wrong, in words for a chess player (android-ui.md §23.4).
@Composable
fun onlineProblemText(f: OnlineFailure): String {
    val site = f.site?.let(::siteName) ?: "—"
    return when (f.problem) {
        OnlineProblem.NETWORK -> stringResource(R.string.online_err_network, site)
        OnlineProblem.RATE_LIMITED -> stringResource(R.string.online_err_rate, site)
        OnlineProblem.UNEXPECTED -> stringResource(R.string.online_err_unexpected, site)
        OnlineProblem.DISK -> stringResource(R.string.online_err_disk)
        OnlineProblem.BUSY -> stringResource(R.string.online_err_busy)
    }
}

/// The short form, for the subtitle of a database and the snackbar (§23.3).
fun problemShort(res: android.content.res.Resources, f: OnlineFailure): String {
    val site = f.site?.let(::siteName)
    return when {
        site != null && f.problem == OnlineProblem.NETWORK -> res.getString(R.string.online_short_network, site)
        site != null && f.problem == OnlineProblem.RATE_LIMITED -> res.getString(R.string.online_short_rate, site)
        else -> res.getString(R.string.online_short_failed)
    }
}

@Composable
fun onlineProblemShort(f: OnlineFailure): String = problemShort(androidx.compose.ui.platform.LocalResources.current, f)

/// What a running download is doing: "Lichess: 1 200 of about 6 541 · about 5 min left",
/// "Chess.com: month 12 of 84 · 340 games".
@Composable
fun downloadProgressText(v: DownloadView): String {
    val s = v.state
    val account = v.accounts.getOrNull(s.account.toInt())
    val games = s.accountGames.toLong()
    return when (account?.site) {
        Site.LICHESS -> {
            val total = account.games?.toLong() ?: 0
            if (total > games) {
                val minutes = ceil(lichessSeconds(total - games) / 60.0).toInt().coerceAtLeast(1)
                stringResource(R.string.online_progress_lichess, groupDigits(games.toInt()), groupDigits(total.toInt()), minutes)
            } else {
                stringResource(R.string.online_progress_count, "Lichess", gamesText(games))
            }
        }
        Site.CHESS_COM -> if (s.monthsTotal > 0u) {
            stringResource(R.string.online_progress_chesscom, s.monthsDone.toInt(), s.monthsTotal.toInt(), gamesText(games))
        } else {
            stringResource(R.string.online_progress_count, "Chess.com", gamesText(games))
        }
        null -> stringResource(R.string.online_downloading, groupDigits(s.added.toInt()))
    }
}

/// How a finished download went: "Done: 473 games" the first time, "+12, 1 246 games in
/// all" later.
@Composable
fun downloadResultText(v: DownloadView): String {
    val s = v.state
    s.failure?.let { return onlineProblemText(it) }
    val total = s.total?.toLong() ?: 0
    return when {
        s.cancelled -> stringResource(R.string.online_stopped, gamesText(total))
        total == 0L -> stringResource(R.string.online_no_games)
        s.added == 0UL -> stringResource(R.string.online_no_new, gamesText(total))
        v.fresh -> stringResource(R.string.online_done, gamesText(total))
        else -> stringResource(R.string.online_added, groupDigits(s.added.toInt()), gamesText(total))
    }
}

/// The share of the whole download done, when it is known: each account is an equal part
/// (Lichess counted by its games, Chess.com by months).
fun downloadShare(v: DownloadView): Float? {
    val s = v.state
    val i = s.account.toInt()
    val account = v.accounts.getOrNull(i) ?: return null
    val part = when (account.site) {
        Site.LICHESS -> account.games?.takeIf { it > 0UL }?.let { (s.accountGames.toDouble() / it.toDouble()).toFloat() }
        Site.CHESS_COM -> s.monthsTotal.takeIf { it > 0u }?.let { s.monthsDone.toFloat() / it.toFloat() }
    } ?: return null
    return ((i + part.coerceIn(0f, 1f)) / v.accounts.size).coerceIn(0f, 1f)
}

/// The search of the screen: kept when the phone is turned (android-ui.md §23.2).
class OnlineSearch : ViewModel() {
    var found by mutableStateOf<Map<Site, AccountLookup>?>(null)
    var chosen by mutableStateOf(setOf<Site>())
    var searching by mutableStateOf(false)

    fun find(forLichess: String, forChessCom: String) {
        if (searching) return
        searching = true
        viewModelScope.launch {
            val result = withContext(Dispatchers.IO) {
                val li = async { findAccount(Site.LICHESS, forLichess) }
                val cc = async { findAccount(Site.CHESS_COM, forChessCom) }
                mapOf(Site.LICHESS to li.await(), Site.CHESS_COM to cc.await())
            }
            found = result
            chosen = result.filterValues { it is AccountLookup.Found }.keys
            searching = false
        }
    }

    fun forget() {
        found = null
    }
}

/// A nickname is shown as wrong only for a character it may not have or when it is too
/// long; a short one is still being typed.
private fun badNick(nick: String): Boolean {
    val t = nick.trim()
    return t.length > 30 || t.any { !(it.isLetterOrDigit() && it.code < 128) && it != '_' && it != '-' }
}

/// "Games from Lichess and Chess.com" (android-ui.md §23.2): a nickname, what was found on
/// each site, the download and how it goes.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun OnlineScreen(onBack: () -> Unit, onOpen: (String) -> Unit) {
    val app = app()
    val downloads = app.downloads
    val views by downloads.views.collectAsState()
    val entries by app.library.entries.collectAsState()
    val search: OnlineSearch = viewModel()
    val keyboard = LocalSoftwareKeyboardController.current
    var nick by rememberSaveable { mutableStateOf("") }
    // A different nickname on Chess.com, once asked for.
    var other by rememberSaveable { mutableStateOf<String?>(null) }
    var together by rememberSaveable { mutableStateOf(true) }
    // The databases this screen started downloading; the found accounts are hidden then,
    // until the next search.
    var started by rememberSaveable { mutableStateOf(listOf<String>()) }
    var launched by rememberSaveable { mutableStateOf(false) }

    val nickOk = validNickname(nick)
    val otherOk = other?.let { it.isBlank() || validNickname(it) } ?: true
    fun find() {
        if (!nickOk || !otherOk) return
        keyboard?.hide()
        launched = false
        val forLichess = nick.trim()
        search.find(forLichess, other?.trim()?.takeIf { it.isNotEmpty() } ?: forLichess)
    }

    val found = search.found
    val accounts: List<OnlineAccount> = found.orEmpty()
        .filterKeys { it in search.chosen }
        .values.mapNotNull { (it as? AccountLookup.Found)?.account }
    val merge = together && accounts.size > 1
    // Where the games go, and how many are there already; read again when a download ends.
    val ended = started.count { views[it]?.state?.running == false }
    val targets by produceState(emptyList<Pair<String, Long?>>(), accounts, merge, ended) {
        value = withContext(Dispatchers.IO) {
            if (accounts.isEmpty()) {
                emptyList()
            } else {
                onlineTargets(downloads.dir.path, accounts, merge).map { t ->
                    t.path to (if (t.exists) onlineDatabase(t.path)?.games?.toLong() else null)
                }
            }
        }
    }
    // A database's name as on the list (it may have been renamed), else its file's.
    fun nameOf(path: String): String {
        val file = File(path).name
        return entries.firstOrNull { it.source == Source.Online && it.location == file }?.name ?: file.removeSuffix(".pgn")
    }

    Scaffold(
        topBar = {
            TopAppBar(
                navigationIcon = { IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Rounded.ArrowBack, stringResource(R.string.back)) } },
                title = { Text(stringResource(R.string.online_title)) },
            )
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding)
                .imePadding()
                .verticalScroll(rememberScrollState())
                .navigationBarsPadding()
                .padding(horizontal = 16.dp, vertical = 8.dp),
        ) {
            Text(stringResource(R.string.online_intro), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(16.dp))
            NickField(
                value = nick,
                label = stringResource(if (other == null) R.string.online_nick else R.string.online_nick_lichess),
                bad = badNick(nick),
                onChange = {
                    nick = it
                    search.forget()
                },
                onDone = ::find,
            )
            if (other == null) {
                TextButton(onClick = { other = "" }, contentPadding = PaddingValues(0.dp)) { Text(stringResource(R.string.online_other)) }
            } else {
                Spacer(Modifier.height(8.dp))
                NickField(
                    value = other.orEmpty(),
                    label = stringResource(R.string.online_other_nick),
                    bad = badNick(other.orEmpty()),
                    onChange = {
                        other = it
                        search.forget()
                    },
                    onDone = ::find,
                )
                TextButton(onClick = {
                    other = null
                    search.forget()
                }, contentPadding = PaddingValues(0.dp)) { Text(stringResource(R.string.online_same_nick)) }
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
                if (search.searching) {
                    CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    Spacer(Modifier.size(12.dp))
                    Text(stringResource(R.string.online_searching), style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
                }
                val enabled = nickOk && otherOk && !search.searching
                // After a search the main button of the screen is "Download".
                if (found != null && !launched) {
                    OutlinedButton(onClick = ::find, enabled = enabled) { Text(stringResource(R.string.online_find)) }
                } else {
                    Button(onClick = ::find, enabled = enabled) { Text(stringResource(R.string.online_find)) }
                }
            }

            if (found != null && !launched) {
                Spacer(Modifier.height(16.dp))
                HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                Spacer(Modifier.height(8.dp))
                for (site in listOf(Site.LICHESS, Site.CHESS_COM)) {
                    val r = found[site] ?: continue
                    FoundRow(site, r, site in search.chosen, onToggle = { on -> search.chosen = if (on) search.chosen + site else search.chosen - site }, onRetry = ::find)
                }
                if (found.values.count { it is AccountLookup.Found } > 1 && search.chosen.size > 1) {
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .heightIn(min = 64.dp)
                            .toggleable(value = together, role = Role.Switch, onValueChange = { together = it }),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Column(Modifier.weight(1f)) {
                            Text(stringResource(R.string.online_merge), style = MaterialTheme.typography.bodyLarge)
                            Text(stringResource(R.string.online_merge_hint), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        Switch(checked = together, onCheckedChange = null)
                    }
                }
                for ((path, games) in targets) {
                    val name = nameOf(path)
                    Text(
                        if (games != null) {
                            stringResource(R.string.online_into_existing, name, gamesText(games))
                        } else {
                            stringResource(R.string.online_into_new, name)
                        },
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(vertical = 4.dp),
                    )
                }
                if (accounts.isNotEmpty()) {
                    Row(Modifier.fillMaxWidth().padding(top = 8.dp), horizontalArrangement = Arrangement.End) {
                        Button(onClick = {
                            val groups = onlineTargets(downloads.dir.path, accounts, merge)
                            started = groups.map { downloads.start(it.accounts, it.path).id }
                            launched = true
                        }) { Text(stringResource(R.string.online_download)) }
                    }
                }
            }

            // The downloads started here: how they go, then how they went.
            for (id in started) {
                val v = views[id] ?: continue
                Spacer(Modifier.height(16.dp))
                DownloadCard(
                    name = entries.firstOrNull { it.id == id }?.name ?: v.accounts.joinToString(" + ") { it.username },
                    view = v,
                    onStop = { downloads.cancel(id) },
                    onOpen = if (!v.state.running && (v.state.total ?: 0UL) > 0UL && entries.any { it.id == id }) ({ onOpen(id) }) else null,
                )
            }
        }
    }
}

@Composable
private fun NickField(value: String, label: String, bad: Boolean, onChange: (String) -> Unit, onDone: () -> Unit) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        label = { Text(label) },
        placeholder = { Text(stringResource(R.string.online_nick_hint)) },
        singleLine = true,
        isError = bad,
        supportingText = if (bad) ({ Text(stringResource(R.string.online_nick_rule)) }) else null,
        keyboardOptions = KeyboardOptions(
            capitalization = KeyboardCapitalization.None,
            autoCorrectEnabled = false,
            keyboardType = KeyboardType.Ascii,
            imeAction = ImeAction.Search,
        ),
        keyboardActions = KeyboardActions(onSearch = { onDone() }),
        modifier = Modifier.fillMaxWidth(),
    )
}

/// What one site gave for the nickname: the account to tick, "no such player", or why it
/// did not answer.
@Composable
private fun FoundRow(site: Site, r: AccountLookup, checked: Boolean, onToggle: (Boolean) -> Unit, onRetry: () -> Unit) {
    when (r) {
        is AccountLookup.Found -> {
            val a = r.account
            val lichess = a.games?.toLong()
            val detail = when {
                lichess != null -> {
                    val upTo = pluralStringResource(R.plurals.games_up_to, lichess.coerceAtMost(Int.MAX_VALUE.toLong()).toInt(), groupDigits(lichess.toInt()))
                    val secs = lichessSeconds(lichess)
                    if (secs < 60) {
                        stringResource(R.string.online_lichess_quick, upTo)
                    } else {
                        stringResource(R.string.online_lichess_found, upTo, ceil(secs / 60.0).toInt())
                    }
                }
                a.months != null -> stringResource(R.string.online_chesscom_found, a.months!!.toInt())
                else -> ""
            }
            Row(
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = 64.dp)
                    .toggleable(value = checked, role = Role.Checkbox, onValueChange = onToggle),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Checkbox(checked = checked, onCheckedChange = null)
                Column(Modifier.weight(1f).padding(start = 12.dp)) {
                    Text("${siteName(site)} · ${a.username}", style = MaterialTheme.typography.bodyLarge)
                    if (detail.isNotEmpty()) {
                        Text(detail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        }
        AccountLookup.NotFound -> Text(
            stringResource(R.string.online_not_found, siteName(site)),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.heightIn(min = 48.dp).padding(start = 36.dp, top = 12.dp),
        )
        is AccountLookup.Failed -> Column(Modifier.padding(start = 36.dp, top = 8.dp)) {
            Text(onlineProblemText(r.failure), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error)
            TextButton(onClick = onRetry, contentPadding = PaddingValues(0.dp)) { Text(stringResource(R.string.online_try_again)) }
        }
    }
}

/// A download started on this screen: its progress and Stop, then its result and Open.
@Composable
private fun DownloadCard(name: String, view: DownloadView, onStop: () -> Unit, onOpen: (() -> Unit)?) {
    val s = view.state
    Column(Modifier.fillMaxWidth()) {
        Text(name, style = MaterialTheme.typography.titleMedium, modifier = Modifier.semantics { heading() })
        Spacer(Modifier.height(4.dp))
        if (s.running) {
            // Not a live region: it changes a few times a second (TalkBack would not stop).
            Text(downloadProgressText(view), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(8.dp))
            val share = downloadShare(view)
            if (share != null) {
                LinearProgressIndicator(progress = { share }, modifier = Modifier.fillMaxWidth())
            } else {
                LinearProgressIndicator(Modifier.fillMaxWidth())
            }
            Spacer(Modifier.height(8.dp))
            Text(stringResource(R.string.online_leave), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                OutlinedButton(onClick = onStop, modifier = Modifier.padding(top = 8.dp)) { Text(stringResource(R.string.online_stop)) }
            }
        } else {
            Text(
                downloadResultText(view),
                style = MaterialTheme.typography.bodyMedium,
                color = if (s.failure != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
            )
            if (onOpen != null) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                    Button(onClick = onOpen, modifier = Modifier.padding(top = 8.dp)) { Text(stringResource(R.string.online_open)) }
                }
            }
        }
    }
}
