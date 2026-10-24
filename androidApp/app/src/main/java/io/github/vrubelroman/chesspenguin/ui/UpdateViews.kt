package io.github.vrubelroman.chesspenguin.ui

import android.content.Intent
import android.net.Uri
import android.provider.Settings
import android.text.format.DateUtils
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowRight
import androidx.compose.material.icons.rounded.Close
import androidx.compose.foundation.layout.offset
import androidx.compose.material3.IconButton
import androidx.compose.material.icons.rounded.SystemUpdateAlt
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.vrubelroman.chesspenguin.BuildConfig
import io.github.vrubelroman.chesspenguin.R
import io.github.vrubelroman.chesspenguin.data.CheckState
import io.github.vrubelroman.chesspenguin.data.NetState
import io.github.vrubelroman.chesspenguin.data.UpdateMessage
import io.github.vrubelroman.chesspenguin.data.UpdateProblem
import io.github.vrubelroman.chesspenguin.data.UpdateState
import io.github.vrubelroman.chesspenguin.data.UpdateStep
import io.github.vrubelroman.chesspenguin.theme.Tabular

private const val SITE_URL = "https://chess.vrubel.xyz"

private fun mb(bytes: Long) = ((bytes + 999_999) / 1_000_000).toInt()

/// The update the card on «Databases» shows, or null: none known, offers turned off (its
/// ×, or Settings), or the check turned off (unless the user asked for it).
@Composable
fun shownUpdate(): UpdateState? {
    val updates = app().updates
    val state by updates.state.collectAsState()
    val auto by updates.auto.collectAsState()
    val offer by updates.offer.collectAsState()
    val asked by updates.asked.collectAsState()
    val s = state ?: return null
    if (!offer) return null
    if (!auto && !asked && s is UpdateState.Available) return null
    return s
}

/// The card over the list of databases (android-ui.md §25.2): the new version, one
/// button, and × that stops the offers (they come back in Settings → About).
@Composable
fun UpdateCard(state: UpdateState, onDismissed: () -> Unit, modifier: Modifier = Modifier) {
    val updates = app().updates
    val update = state.update
    val title = stringResource(R.string.upd_title, update.version)
    val progressOnly = state is UpdateState.Downloading || state is UpdateState.Busy
    val downloadingText = when (state) {
        is UpdateState.Downloading -> if (state.waitingNetwork) {
            title + ", " + stringResource(R.string.upd_waiting_net_short)
        } else {
            stringResource(R.string.upd_card_downloading, update.version, mb(state.done), mb(update.apkSize))
        }
        is UpdateState.Busy -> title + ", " + stepText(state.step)
        else -> null
    }
    Surface(
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
        shape = RoundedCornerShape(12.dp),
        modifier = modifier
            .fillMaxWidth()
            .padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 8.dp)
            .then(
                if (progressOnly) {
                    Modifier
                        .clickable(role = Role.Button) { updates.openSheet() }
                        .clearAndSetSemantics {
                            contentDescription = downloadingText ?: title
                            role = Role.Button
                            onClick { updates.openSheet(); true }
                        }
                } else {
                    Modifier
                },
            ),
    ) {
        Column(Modifier.padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = if (progressOnly) 16.dp else 8.dp)) {
            Row(verticalAlignment = Alignment.Top) {
                Icon(Icons.Rounded.SystemUpdateAlt, null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(24.dp))
                Spacer(Modifier.width(16.dp))
                Column(Modifier.weight(1f)) {
                    Text(
                        title,
                        style = MaterialTheme.typography.bodyLarge,
                        fontWeight = FontWeight.Medium,
                        modifier = Modifier.semantics { heading() },
                    )
                    Spacer(Modifier.height(2.dp))
                    when (state) {
                        is UpdateState.Available, is UpdateState.NeedsPermission -> Text(
                            stringResource(R.string.upd_you_have, BuildConfig.VERSION_NAME),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        is UpdateState.Downloading -> {
                            Spacer(Modifier.height(6.dp))
                            LinearProgressIndicator(
                                progress = { state.done.toFloat() / update.apkSize.coerceAtLeast(1) },
                                modifier = Modifier.fillMaxWidth().height(4.dp),
                            )
                            Spacer(Modifier.height(6.dp))
                            Text(
                                if (state.waitingNetwork) stringResource(R.string.upd_waiting_net_short) else stringResource(R.string.downloaded_of, mb(state.done), mb(update.apkSize)),
                                style = MaterialTheme.typography.bodyMedium.merge(Tabular),
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        is UpdateState.Busy -> {
                            Spacer(Modifier.height(6.dp))
                            LinearProgressIndicator(Modifier.fillMaxWidth().height(4.dp))
                            Spacer(Modifier.height(6.dp))
                            Text(stepText(state.step), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        is UpdateState.Ready -> Text(
                            stringResource(R.string.upd_ready_short),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        is UpdateState.Failed -> Text(
                            shortProblem(state.problem),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.error,
                        )
                    }
                }
                if (progressOnly) {
                    // The whole card opens the sheet.
                    Icon(Icons.AutoMirrored.Rounded.KeyboardArrowRight, null, tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(24.dp))
                } else {
                    IconButton(
                        onClick = {
                            updates.setOffer(false)
                            onDismissed()
                        },
                        modifier = Modifier.offset(x = 12.dp, y = (-12).dp),
                    ) {
                        Icon(Icons.Rounded.Close, stringResource(R.string.upd_dismiss_cd), tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(20.dp))
                    }
                }
            }
            if (!progressOnly) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                    when (state) {
                        is UpdateState.Ready -> FilledTonalButton(onClick = {
                            updates.openSheet()
                            updates.install()
                        }) { Text(stringResource(R.string.upd_install)) }
                        is UpdateState.Failed -> FilledTonalButton(onClick = { updates.openSheet() }) { Text(stringResource(R.string.upd_more)) }
                        else -> FilledTonalButton(onClick = {
                            updates.openSheet()
                            updates.start()
                        }) { Text(stringResource(R.string.upd_update)) }
                    }
                }
            }
        }
    }
}

@Composable
private fun stepText(step: UpdateStep): String = stringResource(
    when (step) {
        UpdateStep.CheckingFile -> R.string.upd_checking_file
        UpdateStep.KeepingEngine -> R.string.upd_keeping_engine
        UpdateStep.Installing -> R.string.upd_installing
    },
)

@Composable
private fun shortProblem(p: UpdateProblem): String = stringResource(
    when (p) {
        UpdateProblem.NoInternet -> R.string.upd_short_offline
        UpdateProblem.NoSpace, UpdateProblem.InstallStorage -> R.string.upd_short_space
        UpdateProblem.Corrupt -> R.string.upd_short_corrupt
        UpdateProblem.DownloadFailed -> R.string.upd_short_failed
        UpdateProblem.BadRelease -> R.string.upd_short_release
        UpdateProblem.Conflict, UpdateProblem.Blocked, UpdateProblem.InstallFailed -> R.string.upd_short_install
    },
)

/// The «Update» sheet (android-ui.md §25.3), over whatever screen is shown.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun UpdateSheet() {
    val app = app()
    val updates = app.updates
    val state by updates.state.collectAsState()
    val net by app.engineFiles.state.collectAsState()
    val games by app.downloads.views.collectAsState()
    val context = LocalContext.current
    val uri = LocalUriHandler.current
    val s = state
    if (s == null) {
        LaunchedEffect(Unit) { updates.closeSheet() }
        return
    }
    val update = s.update
    // Back from the settings with the permission: TalkBack says the download has begun.
    val view = androidx.compose.ui.platform.LocalView.current
    val granted = stringResource(R.string.upd_perm_granted)
    var waited by remember { mutableStateOf(false) }
    LaunchedEffect(s is UpdateState.NeedsPermission, s is UpdateState.Downloading) {
        if (s is UpdateState.NeedsPermission) waited = true
        if (s is UpdateState.Downloading && waited) {
            waited = false
            @Suppress("DEPRECATION")
            view.announceForAccessibility(granted)
        }
    }
    val close = {
        updates.closeSheet()
        updates.dismissProblem()
    }
    ModalBottomSheet(onDismissRequest = close, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(start = 24.dp, end = 24.dp, bottom = 24.dp),
        ) {
            Text(stringResource(R.string.upd_title, update.version), style = MaterialTheme.typography.titleLarge, modifier = Modifier.semantics { heading() })
            Spacer(Modifier.height(4.dp))
            Text(
                stringResource(R.string.upd_you_have, BuildConfig.VERSION_NAME),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(16.dp))
            // The engine stays when there is one to keep: in the app's storage, or in a full
            // build that keeps it before the install.
            val engine = net == NetState.Ready || BuildConfig.NET_BUILT_IN
            Text(stringResource(if (engine) R.string.upd_keeps else R.string.upd_keeps_no_engine), style = MaterialTheme.typography.bodyLarge)
            Text(stringResource(R.string.upd_closes), style = MaterialTheme.typography.bodyLarge)
            if (games.values.any { it.state.running }) {
                Spacer(Modifier.height(8.dp))
                Text(stringResource(R.string.upd_online_note), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Spacer(Modifier.height(16.dp))
            val live = Modifier.semantics { liveRegion = LiveRegionMode.Polite }
            when (s) {
                is UpdateState.Available -> Buttons(
                    secondary = stringResource(R.string.not_now) to close,
                    primary = stringResource(R.string.upd_update) to { updates.start() },
                )
                is UpdateState.NeedsPermission -> {
                    val openSettings = {
                        updates.permissionAsked()
                        context.startActivity(
                            Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${context.packageName}")),
                        )
                    }
                    if (!s.denied) {
                        Text(stringResource(R.string.upd_perm_title), style = MaterialTheme.typography.titleMedium, modifier = live)
                        Spacer(Modifier.height(4.dp))
                        Text(stringResource(R.string.upd_perm_body), style = MaterialTheme.typography.bodyMedium)
                    } else {
                        Text(stringResource(R.string.upd_perm_denied), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error, modifier = live)
                        Link(stringResource(R.string.upd_on_site)) { uri.openUri(SITE_URL) }
                    }
                    Spacer(Modifier.height(24.dp))
                    Buttons(
                        secondary = stringResource(R.string.cancel) to close,
                        primary = stringResource(R.string.upd_perm_open) to openSettings,
                    )
                }
                is UpdateState.Downloading -> {
                    LinearProgressIndicator(
                        progress = { s.done.toFloat() / update.apkSize.coerceAtLeast(1) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(Modifier.height(8.dp))
                    Text(
                        if (s.waitingNetwork) stringResource(R.string.upd_waiting_net) else stringResource(R.string.downloaded_of, mb(s.done), mb(update.apkSize)),
                        style = MaterialTheme.typography.bodyMedium.merge(Tabular),
                    )
                    Text(stringResource(R.string.upd_leave), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = live)
                    Spacer(Modifier.height(16.dp))
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                        TextButton(onClick = {
                            updates.cancel()
                            updates.closeSheet()
                        }) { Text(stringResource(R.string.cancel_download)) }
                    }
                }
                is UpdateState.Busy -> {
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                    Spacer(Modifier.height(8.dp))
                    Text(stepText(s.step), style = MaterialTheme.typography.bodyMedium, modifier = live)
                }
                is UpdateState.Ready -> {
                    Text(
                        stringResource(if (s.aborted) R.string.upd_aborted else R.string.upd_ready),
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = live,
                    )
                    Spacer(Modifier.height(24.dp))
                    Buttons(
                        secondary = stringResource(R.string.not_now) to { updates.closeSheet() },
                        primary = stringResource(R.string.upd_install) to { updates.install() },
                    )
                }
                is UpdateState.Failed -> Problem(s, onRetry = { updates.start() }, onSite = { uri.openUri(SITE_URL) }, onClose = close)
            }
        }
    }
}

@Composable
private fun Problem(s: UpdateState.Failed, onRetry: () -> Unit, onSite: () -> Unit, onClose: () -> Unit) {
    val updates = app().updates
    val text = when (s.problem) {
        UpdateProblem.NoInternet -> stringResource(R.string.no_internet)
        UpdateProblem.DownloadFailed -> stringResource(R.string.upd_err_failed)
        UpdateProblem.NoSpace -> stringResource(R.string.no_space, mb(updates.needed(s.update)))
        UpdateProblem.Corrupt -> stringResource(R.string.upd_err_corrupt)
        UpdateProblem.BadRelease -> stringResource(R.string.upd_err_release)
        UpdateProblem.InstallStorage -> stringResource(R.string.upd_err_storage, mb(updates.needed(s.update)))
        UpdateProblem.Conflict -> stringResource(R.string.upd_err_conflict)
        UpdateProblem.Blocked -> stringResource(R.string.upd_err_blocked)
        UpdateProblem.InstallFailed -> stringResource(R.string.upd_err_install)
    }
    Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
    val install = s.problem in setOf(UpdateProblem.InstallStorage, UpdateProblem.Conflict, UpdateProblem.Blocked, UpdateProblem.InstallFailed)
    val detail = s.detail
    if (install && !detail.isNullOrBlank()) {
        var open by rememberSaveable { mutableStateOf(false) }
        Link(stringResource(R.string.upd_details)) { open = !open }
        if (open) {
            @Suppress("DEPRECATION")
            val clipboard = androidx.compose.ui.platform.LocalClipboardManager.current
            Text(detail, fontFamily = FontFamily.Monospace, fontSize = 12.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
            TextButton(onClick = { clipboard.setText(androidx.compose.ui.text.AnnotatedString(detail)) }) { Text(stringResource(R.string.upd_copy)) }
        }
    }
    Spacer(Modifier.height(24.dp))
    val notNow = stringResource(R.string.not_now)
    val ok = stringResource(R.string.ok)
    val retry = stringResource(R.string.retry)
    val site = stringResource(R.string.upd_open_site)
    when (s.problem) {
        UpdateProblem.NoInternet, UpdateProblem.DownloadFailed, UpdateProblem.InstallStorage -> Buttons(notNow to onClose, retry to onRetry)
        UpdateProblem.Corrupt -> Buttons(notNow to onClose, stringResource(R.string.download_again) to onRetry)
        UpdateProblem.NoSpace -> Buttons(null, ok to onClose)
        UpdateProblem.BadRelease, UpdateProblem.Conflict, UpdateProblem.Blocked -> Buttons(ok to onClose, site to onSite)
        UpdateProblem.InstallFailed -> Buttons(site to onSite, retry to onRetry)
    }
}

@Composable
private fun Buttons(secondary: Pair<String, () -> Unit>?, primary: Pair<String, () -> Unit>) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
        secondary?.let { (label, action) -> TextButton(onClick = action) { Text(label) } }
        Button(onClick = primary.second) { Text(primary.first) }
    }
}

@Composable
private fun Link(text: String, onClick: () -> Unit) {
    Row(
        Modifier.heightIn(min = 48.dp).clickable(role = Role.Button, onClick = onClick),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.primary)
    }
}

/// The «Updates» block of About (android-ui.md §25.5).
@Composable
fun AboutUpdates() {
    val updates = app().updates
    val state by updates.state.collectAsState()
    val check by updates.check.collectAsState()
    val auto by updates.auto.collectAsState()
    val offer by updates.offer.collectAsState()
    val last by updates.lastCheck.collectAsState()
    val context = LocalContext.current
    Column(Modifier.fillMaxWidth()) {
        Text(
            stringResource(R.string.upd_group),
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.semantics { heading() },
        )
        Spacer(Modifier.height(8.dp))
        val s = state
        if (s == null) {
            // Said only after a check that got an answer.
            if (last > 0 || check == CheckState.Latest) Text(stringResource(R.string.upd_latest), style = MaterialTheme.typography.bodyLarge)
        } else {
            Text(stringResource(R.string.upd_title, s.update.version), style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Medium)
            Text(
                stringResource(R.string.upd_you_have, BuildConfig.VERSION_NAME),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            when (s) {
                is UpdateState.Downloading -> {
                    LinearProgressIndicator(progress = { s.done.toFloat() / s.update.apkSize.coerceAtLeast(1) }, modifier = Modifier.fillMaxWidth())
                    Text(
                        if (s.waitingNetwork) stringResource(R.string.upd_waiting_net_short) else stringResource(R.string.downloaded_of, mb(s.done), mb(s.update.apkSize)),
                        style = MaterialTheme.typography.bodyMedium.merge(Tabular),
                    )
                }
                is UpdateState.Busy -> {
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                    Text(stepText(s.step), style = MaterialTheme.typography.bodyMedium)
                }
                is UpdateState.Failed -> Text(shortProblem(s.problem), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error)
                else -> {}
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                when (s) {
                    is UpdateState.Ready -> Button(onClick = {
                        updates.openSheet()
                        updates.install()
                    }) { Text(stringResource(R.string.upd_install)) }
                    is UpdateState.Downloading, is UpdateState.Busy -> TextButton(onClick = { updates.openSheet() }) { Text(stringResource(R.string.upd_more)) }
                    is UpdateState.Failed -> TextButton(onClick = { updates.openSheet() }) { Text(stringResource(R.string.upd_more)) }
                    else -> Button(onClick = {
                        updates.openSheet()
                        updates.start()
                    }) { Text(stringResource(R.string.upd_update)) }
                }
            }
        }
        // The card on «Databases» (its × turns this off); the check below goes on either way.
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 72.dp)
                .toggleable(value = offer, enabled = auto, role = Role.Switch, onValueChange = { updates.setOffer(it) }),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(stringResource(R.string.upd_offer), style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
            Switch(checked = offer, enabled = auto, onCheckedChange = null)
        }
        Text(
            stringResource(if (auto) R.string.upd_offer_sub else R.string.upd_offer_needs_check),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 72.dp)
                .toggleable(value = auto, role = Role.Switch, onValueChange = { updates.setAuto(it) }),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(stringResource(R.string.upd_auto), style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
            Switch(checked = auto, onCheckedChange = null)
        }
        Text(stringResource(R.string.upd_auto_sub), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (check == CheckState.Checking) {
            Row(Modifier.heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                Spacer(Modifier.width(12.dp))
                Text(stringResource(R.string.upd_checking), style = MaterialTheme.typography.bodyMedium)
            }
        } else {
            // Not while an update is downloaded or installed: the check waits for it.
            val busy = s is UpdateState.Downloading || s is UpdateState.Busy || s is UpdateState.NeedsPermission
            TextButton(onClick = { updates.checkNow() }, enabled = !busy, contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp)) {
                Text(stringResource(R.string.upd_check_now))
            }
        }
        when (check) {
            CheckState.Offline -> Text(stringResource(R.string.upd_check_offline), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error)
            CheckState.Failed -> Text(stringResource(R.string.upd_check_failed), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error)
            else -> {}
        }
        if (last > 0) {
            // «today, 14:05» inside the sentence: Android writes it from a capital.
            val whenText = DateUtils.getRelativeDateTimeString(context, last, DateUtils.DAY_IN_MILLIS, DateUtils.WEEK_IN_MILLIS, 0)
                .toString().replaceFirstChar { it.lowercaseChar() }
            Text(stringResource(R.string.upd_checked, whenText), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/// The second line of «About» in Settings while a newer version is known (§25.5).
@Composable
fun aboutUpdateLine(): String? {
    val updates = app().updates
    val state by updates.state.collectAsState()
    val auto by updates.auto.collectAsState()
    val asked by updates.asked.collectAsState()
    val s = state ?: return null
    if (!auto && !asked && s is UpdateState.Available) return null
    return when (s) {
        is UpdateState.Downloading, is UpdateState.Busy -> stringResource(R.string.s_about_downloading, s.update.version)
        is UpdateState.Ready -> stringResource(R.string.s_about_ready, s.update.version)
        else -> stringResource(R.string.s_about_available, s.update.version)
    }
}

/// Shows the update's messages in a screen's snackbar (only one screen is on at a time).
@Composable
fun UpdateMessages(snackbar: SnackbarHostState) {
    val updates = app().updates
    val message by updates.message.collectAsState()
    val ready = stringResource(R.string.upd_ready_snack)
    val install = stringResource(R.string.upd_install)
    val cancelled = stringResource(R.string.download_cancelled)
    val latest = stringResource(R.string.upd_latest)
    val doneText = (message as? UpdateMessage.Updated)?.let { stringResource(R.string.upd_done_snack, it.from, it.to) }
    // Marked shown when its snackbar is gone: a screen left meanwhile hands it to the next.
    LaunchedEffect(message) {
        val m = message ?: return@LaunchedEffect
        snackbar.currentSnackbarData?.dismiss()
        when (m) {
            UpdateMessage.Ready -> {
                if (snackbar.showSnackbar(ready, install, duration = SnackbarDuration.Long) == SnackbarResult.ActionPerformed) {
                    updates.openSheet()
                    updates.install()
                }
            }
            UpdateMessage.Cancelled -> snackbar.showSnackbar(cancelled)
            UpdateMessage.Latest -> snackbar.showSnackbar(latest)
            is UpdateMessage.Updated -> snackbar.showSnackbar(doneText ?: "", duration = SnackbarDuration.Long)
        }
        updates.shown(m)
    }
}
