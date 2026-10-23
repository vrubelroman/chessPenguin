package io.github.vrubelroman.chesspenguin.data

import android.app.DownloadManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.Build
import android.os.storage.StorageManager
import io.github.vrubelroman.chesspenguin.BuildConfig
import io.github.vrubelroman.chesspenguin.ChessPenguinApp
import io.github.vrubelroman.chesspenguin.R
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import uniffi.mobile_api.ReleaseCheck
import uniffi.mobile_api.checkRelease
import uniffi.mobile_api.releaseApi
import java.io.File
import java.io.IOException
import java.security.MessageDigest

/// A newer release, as it is kept between starts: its light APK is what is installed.
@Serializable
data class Update(
    val version: String,
    val page: String,
    val apkName: String,
    val apkUrl: String,
    val apkSize: Long,
    val sha256: String,
)

/// A step that takes seconds and cannot be stopped.
enum class UpdateStep { CheckingFile, KeepingEngine, Installing }

/// Why an update did not go on (android-ui.md §25.3 F).
enum class UpdateProblem { NoInternet, DownloadFailed, NoSpace, Corrupt, BadRelease, InstallStorage, Conflict, Blocked, InstallFailed }

/// Where the update of a known newer release is.
sealed interface UpdateState {
    val update: Update

    data class Available(override val update: Update) : UpdateState

    /// Android needs the user's permission to install apps; `denied`: back without it.
    data class NeedsPermission(override val update: Update, val denied: Boolean) : UpdateState

    data class Downloading(override val update: Update, val done: Long, val waitingNetwork: Boolean) : UpdateState

    data class Busy(override val update: Update, val step: UpdateStep) : UpdateState

    /// Downloaded and checked; `aborted`: the user said no to Android's question.
    data class Ready(override val update: Update, val aborted: Boolean) : UpdateState

    data class Failed(override val update: Update, val problem: UpdateProblem, val detail: String?) : UpdateState
}

/// The manual check in About.
sealed interface CheckState {
    data object Idle : CheckState
    data object Checking : CheckState
    data object Latest : CheckState
    data object Offline : CheckState
    data object Failed : CheckState
}

/// Messages for a snackbar over the app.
sealed interface UpdateMessage {
    data object Ready : UpdateMessage
    data object Cancelled : UpdateMessage
    data object Latest : UpdateMessage
    data class Updated(val from: String, val to: String, val page: String) : UpdateMessage
}

/// The app updating itself (android-ui.md §25): asks GitHub for a newer release (the
/// core's check, the same as the desktop's), downloads its light APK with the system's
/// download service, checks it against the release's SHA-256 and gives it to Android's
/// package installer. What is known is kept, so a restart picks it up where it was.
class AppUpdates(private val context: Context, private val scope: CoroutineScope, private val engine: EngineFiles) {
    private val prefs = context.getSharedPreferences("update", Context.MODE_PRIVATE)
    private val downloads get() = context.getSystemService(DownloadManager::class.java)
    private val dir get() = context.getExternalFilesDir("updates")
    private val json = Json { ignoreUnknownKeys = true }
    private val api = BuildConfig.UPDATE_API.ifEmpty { releaseApi() }

    private val _state = MutableStateFlow<UpdateState?>(null)
    /// The known newer release and where its update is; null when there is none.
    val state: StateFlow<UpdateState?> = _state.asStateFlow()

    private val _check = MutableStateFlow<CheckState>(CheckState.Idle)
    val check: StateFlow<CheckState> = _check.asStateFlow()

    private val _auto = MutableStateFlow(prefs.getBoolean(AUTO, !BuildConfig.DEBUG))
    /// «Check for updates at start».
    val auto: StateFlow<Boolean> = _auto.asStateFlow()

    private val _hidden = MutableStateFlow(prefs.getString(HIDDEN, null))
    /// The version hidden with «Not now» on the card.
    val hidden: StateFlow<String?> = _hidden.asStateFlow()

    /// The release was found by «Check now»: its card shows even with the check off.
    private val _asked = MutableStateFlow(false)
    val asked: StateFlow<Boolean> = _asked.asStateFlow()

    private val _lastCheck = MutableStateFlow(prefs.getLong(LAST_OK, 0L))
    /// When the last check that got an answer was made (0: never).
    val lastCheck: StateFlow<Long> = _lastCheck.asStateFlow()

    private val _message = MutableStateFlow<UpdateMessage?>(null)
    /// One message for a snackbar; the screen that shows it calls [shown].
    val message: StateFlow<UpdateMessage?> = _message.asStateFlow()

    fun shown(m: UpdateMessage) {
        _message.compareAndSet(m, null)
        if (m is UpdateMessage.Updated) prefs.edit().remove(UPDATED_FROM).remove(UPDATED_PAGE).apply()
    }

    private val _confirm = MutableStateFlow<Intent?>(null)
    /// Android's question «Update this app?», to be shown when the app is on the screen.
    val confirm: StateFlow<Intent?> = _confirm.asStateFlow()

    private val _sheet = MutableStateFlow(prefs.getString(AWAITING_PERMISSION, null) != null)
    /// The update sheet is open: a finished download goes on to install by itself. It
    /// opens again after a restart while the user was in the permission's settings.
    val sheet: StateFlow<Boolean> = _sheet.asStateFlow()
    /// The app is on the screen (MainActivity's onStart … onStop).
    @Volatile var foreground = false

    /// The user watches the sheet: a finished download goes on to install. Not when the
    /// app is in the background — Android would close it unseen (§25.1 p. 6).
    private val watched get() = _sheet.value && foreground

    fun openSheet() {
        _sheet.value = true
    }

    fun closeSheet() {
        _sheet.value = false
    }

    private var watching: Job? = null
    private var session: Int? = null
    private var confirmShown = false

    init {
        // The first start of a new version: say so, and forget the update.
        // Kept until a screen shows it: Android may start the new version in the background
        // first, only to hand it the installer's answer.
        prefs.getString(INSTALLING_FROM, null)?.let { from ->
            if (from != BuildConfig.VERSION_NAME) {
                prefs.edit()
                    .putString(UPDATED_FROM, from)
                    .putString(UPDATED_PAGE, known()?.page ?: "")
                    .remove(LATEST)
                    .remove(READY)
                    .apply()
                forgetFile()
            }
            prefs.edit().remove(INSTALLING_FROM).apply()
        }
        prefs.getString(UPDATED_FROM, null)?.let { from ->
            _message.value = UpdateMessage.Updated(from, BuildConfig.VERSION_NAME, prefs.getString(UPDATED_PAGE, "") ?: "")
        }
        val update = known()
        when {
            // Installed by hand from the site, or never newer than this app.
            update == null || !newer(update.version) -> forgetAll()
            prefs.getLong(DOWNLOAD, -1L) != -1L -> watch(update, prefs.getLong(DOWNLOAD, -1L))
            update.sha256.isEmpty() -> _state.value = UpdateState.Failed(update, UpdateProblem.BadRelease, null)
            prefs.getString(READY, null) == update.version && apk(update).exists() -> _state.value = UpdateState.Ready(update, aborted = false)
            else -> {
                forgetFile()
                _state.value = UpdateState.Available(update)
            }
        }
    }

    /// On start and on return to the app: a check when one is due (§25.6).
    fun checkIfDue() {
        if (!_auto.value || busy()) return
        val now = System.currentTimeMillis()
        if (now - _lastCheck.value < DAY || now - prefs.getLong(LAST_ATTEMPT, 0L) < HOUR) return
        scope.launch { runCheck(manual = false) }
    }

    /// «Check now» (works with the automatic check off).
    fun checkNow() {
        if (_check.value == CheckState.Checking) return
        scope.launch { runCheck(manual = true) }
    }

    private suspend fun runCheck(manual: Boolean) {
        if (busy()) {
            if (manual) _check.value = CheckState.Idle
            return
        }
        _check.value = CheckState.Checking
        prefs.edit().putLong(LAST_ATTEMPT, System.currentTimeMillis()).apply()
        if (!online()) {
            _check.value = if (manual) CheckState.Offline else CheckState.Idle
            return
        }
        val result = withContext(Dispatchers.IO) { checkRelease(api) }
        when (result) {
            is ReleaseCheck.Newer -> {
                val light = result.release.light ?: return
                val update = Update(result.release.version, result.release.page, light.name, light.url, light.size.toLong(), light.sha256)
                checked()
                _check.value = CheckState.Idle
                if (manual) {
                    _asked.value = true
                    setHidden(null)
                }
                found(update)
            }
            is ReleaseCheck.Unverifiable -> {
                checked()
                _check.value = CheckState.Idle
                val light = result.release.light ?: return
                val update = Update(result.release.version, result.release.page, light.name, light.url, light.size.toLong(), "")
                if (manual) {
                    _asked.value = true
                    setHidden(null)
                }
                found(update)
                if (!busy()) _state.value = UpdateState.Failed(update, UpdateProblem.BadRelease, null)
            }
            is ReleaseCheck.UpToDate -> {
                checked()
                _check.value = CheckState.Latest
                if (manual) _message.value = UpdateMessage.Latest
                if (!busy()) forgetAll()
            }
            is ReleaseCheck.Failed -> _check.value = if (manual) CheckState.Failed else CheckState.Idle
        }
    }

    private fun checked() {
        val now = System.currentTimeMillis()
        _lastCheck.value = now
        prefs.edit().putLong(LAST_OK, now).apply()
    }

    /// A newer release found: a downloaded older one is of no use any more.
    private fun found(update: Update) {
        val current = _state.value
        if (current != null && current.update == update) return
        if (busy()) return
        forgetFile()
        prefs.edit().putString(LATEST, json.encodeToString(update)).remove(READY).apply()
        if (_hidden.value != null && _hidden.value != update.version) setHidden(null)
        _state.value = UpdateState.Available(update)
    }

    fun setAuto(on: Boolean) {
        _auto.value = on
        if (!on) _asked.value = false
        prefs.edit().putBoolean(AUTO, on).apply()
        if (on) checkIfDue()
    }

    /// «Not now» on the card: hidden until a newer version.
    fun hide() {
        _state.value?.let { setHidden(it.update.version) }
    }

    private fun setHidden(version: String?) {
        _hidden.value = version
        prefs.edit().apply { if (version == null) remove(HIDDEN) else putString(HIDDEN, version) }.apply()
    }

    /// «Update» / «Try again» / «Download again»: from the permission on to the install.
    fun start() {
        val update = _state.value?.update ?: return
        if (update.sha256.isEmpty()) {
            _state.value = UpdateState.Failed(update, UpdateProblem.BadRelease, null)
            return
        }
        if (!context.packageManager.canRequestPackageInstalls()) {
            _state.value = UpdateState.NeedsPermission(update, denied = false)
            return
        }
        if (prefs.getString(READY, null) == update.version && apk(update).exists()) {
            install(update)
            return
        }
        download(update)
    }

    /// The permission's settings page is being opened: back from it, the update goes on
    /// (also when Android restarted the app meanwhile).
    fun permissionAsked() {
        _state.value?.let { prefs.edit().putString(AWAITING_PERMISSION, it.update.version).apply() }
    }

    /// Back in the app after the permission's settings page.
    fun onResume() {
        prefs.getString(AWAITING_PERMISSION, null)?.let { version ->
            val update = _state.value?.update
            prefs.edit().remove(AWAITING_PERMISSION).apply()
            if (update != null && update.version == version) {
                if (context.packageManager.canRequestPackageInstalls()) {
                    _sheet.value = true
                    start()
                } else {
                    _state.value = UpdateState.NeedsPermission(update, denied = true)
                }
            }
        }
        // Android's question was closed with no answer: the file waits for «Install».
        if (confirmShown) {
            confirmShown = false
            scope.launch {
                delay(1500)
                val s = _state.value
                if (s is UpdateState.Busy && s.step == UpdateStep.Installing && _confirm.value == null) {
                    abandon()
                    _state.value = UpdateState.Ready(s.update, aborted = true)
                }
            }
        }
    }

    /// The question was put on the screen.
    fun confirmShown() {
        _confirm.value = null
        confirmShown = true
    }

    private fun download(update: Update) {
        if (!online()) {
            _state.value = UpdateState.Failed(update, UpdateProblem.NoInternet, null)
            return
        }
        if (freeSpace() < needed(update)) {
            _state.value = UpdateState.Failed(update, UpdateProblem.NoSpace, null)
            return
        }
        forgetFile()
        // Under another name while it comes: removing the download from the system's list
        // deletes its file, and the checked file must stay.
        val target = File(apk(update).path + ".download")
        val request = DownloadManager.Request(Uri.parse(update.apkUrl))
            .setTitle(context.getString(R.string.upd_notification))
            .setDestinationUri(Uri.fromFile(target))
            // No "complete" notification: tapping it would open the file past the check.
            .setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE)
            .setAllowedOverMetered(true)
            .setAllowedOverRoaming(true)
        val id = try {
            downloads.enqueue(request)
        } catch (e: IllegalArgumentException) {
            _state.value = UpdateState.Failed(update, UpdateProblem.DownloadFailed, e.message)
            return
        }
        prefs.edit().putLong(DOWNLOAD, id).putString(DOWNLOAD_VERSION, update.version).apply()
        _state.value = UpdateState.Downloading(update, 0, waitingNetwork = false)
        watch(update, id)
    }

    /// «Cancel» while downloading.
    fun cancel() {
        val update = _state.value?.update ?: return
        watching?.cancel()
        val id = prefs.getLong(DOWNLOAD, -1L)
        if (id != -1L) downloads.remove(id)
        prefs.edit().remove(DOWNLOAD).remove(DOWNLOAD_VERSION).apply()
        forgetFile()
        _state.value = UpdateState.Available(update)
        _message.value = UpdateMessage.Cancelled
    }

    /// «Not now» in the sheet after an error: the card shows the release again.
    fun dismissProblem() {
        val s = _state.value
        // The same file would fail the same way: the card keeps saying so.
        if (s is UpdateState.Failed && s.problem in setOf(UpdateProblem.Conflict, UpdateProblem.Blocked, UpdateProblem.BadRelease)) return
        if (s is UpdateState.Failed || s is UpdateState.NeedsPermission) {
            prefs.edit().remove(AWAITING_PERMISSION).apply()
            val ready = prefs.getString(READY, null) == s.update.version && apk(s.update).exists()
            _state.value = if (ready) UpdateState.Ready(s.update, aborted = false) else UpdateState.Available(s.update)
        }
    }

    private fun watch(update: Update, id: Long) {
        watching?.cancel()
        _state.value = UpdateState.Downloading(update, 0, waitingNetwork = false)
        watching = scope.launch(Dispatchers.Main.immediate) {
            while (isActive) {
                val row = withContext(Dispatchers.IO) { progress(id) }
                if (row == null) {
                    // Removed from the system's list of downloads.
                    prefs.edit().remove(DOWNLOAD).remove(DOWNLOAD_VERSION).apply()
                    _state.value = UpdateState.Available(update)
                    return@launch
                }
                when (row.status) {
                    DownloadManager.STATUS_SUCCESSFUL -> {
                        val moved = File(apk(update).path + ".download").renameTo(apk(update))
                        downloads.remove(id)
                        prefs.edit().remove(DOWNLOAD).remove(DOWNLOAD_VERSION).apply()
                        if (moved) {
                            verify(update)
                        } else {
                            forgetFile()
                            _state.value = UpdateState.Failed(update, UpdateProblem.DownloadFailed, null)
                        }
                        return@launch
                    }
                    DownloadManager.STATUS_FAILED -> {
                        downloads.remove(id)
                        prefs.edit().remove(DOWNLOAD).remove(DOWNLOAD_VERSION).apply()
                        forgetFile()
                        val space = row.reason == DownloadManager.ERROR_INSUFFICIENT_SPACE
                        _state.value = UpdateState.Failed(update, if (space) UpdateProblem.NoSpace else UpdateProblem.DownloadFailed, null)
                        return@launch
                    }
                    else -> {
                        // A server that does not answer is retried quietly; only a missing network is said.
                        val waiting = row.status == DownloadManager.STATUS_PAUSED && row.reason == DownloadManager.PAUSED_WAITING_FOR_NETWORK
                        _state.value = UpdateState.Downloading(update, row.done.coerceAtLeast(0), waiting)
                    }
                }
                delay(500)
            }
        }
    }

    private class Row(val status: Int, val reason: Int, val done: Long)

    private fun progress(id: Long): Row? =
        downloads.query(DownloadManager.Query().setFilterById(id))?.use { c ->
            if (!c.moveToFirst()) return null
            Row(
                c.getInt(c.getColumnIndexOrThrow(DownloadManager.COLUMN_STATUS)),
                c.getInt(c.getColumnIndexOrThrow(DownloadManager.COLUMN_REASON)),
                c.getLong(c.getColumnIndexOrThrow(DownloadManager.COLUMN_BYTES_DOWNLOADED_SO_FAR)),
            )
        }

    /// The SHA-256 of the file against the release's; then on to the install when the user
    /// waits for it in the sheet, or «downloaded» when they are elsewhere.
    private suspend fun verify(update: Update) {
        _state.value = UpdateState.Busy(update, UpdateStep.CheckingFile)
        val file = apk(update)
        val sum = withContext(Dispatchers.IO) { runCatching { sha256(file) }.getOrNull() }
        if (sum != update.sha256) {
            forgetFile()
            _state.value = UpdateState.Failed(update, UpdateProblem.Corrupt, null)
            return
        }
        prefs.edit().putString(READY, update.version).apply()
        if (watched) {
            install(update)
        } else {
            _state.value = UpdateState.Ready(update, aborted = false)
            // With the sheet open the user sees it there.
            if (!_sheet.value) _message.value = UpdateMessage.Ready
        }
    }

    /// «Install»: the engine's network kept (full build), then Android's installer.
    fun install() {
        val s = _state.value ?: return
        if (!context.packageManager.canRequestPackageInstalls()) {
            start()
            return
        }
        install(s.update)
    }

    private fun install(update: Update) {
        scope.launch(Dispatchers.Main.immediate) {
            if (engine.builtIn) {
                _state.value = UpdateState.Busy(update, UpdateStep.KeepingEngine)
                engine.keepBuiltIn()
            }
            _state.value = UpdateState.Busy(update, UpdateStep.Installing)
            val committed = withContext(Dispatchers.IO) { runCatching { commit(apk(update)) } }
            committed.onFailure { e ->
                abandon()
                val problem = if (e is IOException && freeSpace() < update.apkSize * 2) UpdateProblem.InstallStorage else UpdateProblem.InstallFailed
                _state.value = UpdateState.Failed(update, problem, e.message)
            }
        }
    }

    /// Writes the APK into an installer session and commits it; the answer comes to
    /// [InstallResult].
    private fun commit(file: File) {
        val installer = context.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(context.packageName)
            setSize(file.length())
            // Without Android's question when it allows that (the app installed the last
            // version itself): the conditions are Android's.
            setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
            if (Build.VERSION.SDK_INT >= 33) setPackageSource(PackageInstaller.PACKAGE_SOURCE_DOWNLOADED_FILE)
        }
        val id = installer.createSession(params)
        session = id
        installer.openSession(id).use { s ->
            file.inputStream().use { input ->
                s.openWrite("base.apk", 0, file.length()).use { out ->
                    input.copyTo(out, 256 * 1024)
                    s.fsync(out)
                }
            }
            prefs.edit().putString(INSTALLING_FROM, BuildConfig.VERSION_NAME).apply()
            val intent = Intent(context, InstallResult::class.java).setPackage(context.packageName)
            // Mutable: the installer fills in the result.
            val pending = PendingIntent.getBroadcast(context, id, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE)
            s.commit(pending.intentSender)
        }
    }

    private fun abandon() {
        session?.let { runCatching { context.packageManager.packageInstaller.abandonSession(it) } }
        session = null
        prefs.edit().remove(INSTALLING_FROM).apply()
    }

    /// The installer's answer (from [InstallResult]).
    internal fun onInstallResult(status: Int, message: String?, confirm: Intent?) {
        val update = _state.value?.update ?: return
        when (status) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> _confirm.value = confirm
            // The process ends with the install: nothing more to show.
            PackageInstaller.STATUS_SUCCESS -> {}
            else -> {
                session = null
                prefs.edit().remove(INSTALLING_FROM).apply()
                _confirm.value = null
                _state.value = when (status) {
                    PackageInstaller.STATUS_FAILURE_ABORTED -> UpdateState.Ready(update, aborted = true)
                    PackageInstaller.STATUS_FAILURE_STORAGE -> UpdateState.Failed(update, UpdateProblem.InstallStorage, message)
                    PackageInstaller.STATUS_FAILURE_CONFLICT -> UpdateState.Failed(update, UpdateProblem.Conflict, message)
                    PackageInstaller.STATUS_FAILURE_BLOCKED -> UpdateState.Failed(update, UpdateProblem.Blocked, message)
                    else -> UpdateState.Failed(update, UpdateProblem.InstallFailed, message)
                }
            }
        }
    }

    /// Space the update needs: the file twice (download, install), and the engine's
    /// network when the full build keeps it first.
    fun needed(update: Update): Long =
        2 * update.apkSize + if (engine.builtIn && engine.state.value != NetState.Ready) BuildConfig.NET_SIZE else 0

    /// Downloading, checking or installing: a check waits for it to end.
    private fun busy(): Boolean = _state.value.let { it is UpdateState.Downloading || it is UpdateState.Busy || it is UpdateState.NeedsPermission }

    private fun known(): Update? = prefs.getString(LATEST, null)?.let { runCatching { json.decodeFromString<Update>(it) }.getOrNull() }

    private fun newer(version: String): Boolean = compare(version, BuildConfig.VERSION_NAME) > 0

    private fun apk(update: Update): File = File(dir ?: context.cacheDir, update.apkName)

    private fun forgetFile() {
        dir?.listFiles()?.forEach { it.delete() }
        prefs.edit().remove(READY).apply()
    }

    private fun forgetAll() {
        forgetFile()
        prefs.edit().remove(LATEST).remove(DOWNLOAD).remove(DOWNLOAD_VERSION).remove(AWAITING_PERMISSION).apply()
        _state.value = null
        _asked.value = false
    }

    private fun freeSpace(): Long = try {
        val storage = context.getSystemService(StorageManager::class.java)
        storage.getAllocatableBytes(storage.getUuidForPath(context.filesDir))
    } catch (e: IOException) {
        context.filesDir.usableSpace
    }

    private fun online(): Boolean {
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return true
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
        return caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
    }

    private companion object {
        const val AUTO = "autoCheck"
        const val LAST_OK = "lastCheckOk"
        const val LAST_ATTEMPT = "lastAttempt"
        const val LATEST = "latest"
        const val HIDDEN = "hiddenVersion"
        const val DOWNLOAD = "downloadId"
        const val DOWNLOAD_VERSION = "downloadVersion"
        const val READY = "readyVersion"
        const val AWAITING_PERMISSION = "awaitingPermission"
        const val INSTALLING_FROM = "installingFrom"
        const val UPDATED_FROM = "updatedFrom"
        const val UPDATED_PAGE = "updatedPage"
        const val DAY = 24 * 60 * 60 * 1000L
        const val HOUR = 60 * 60 * 1000L
    }
}

/// "0.15.0" against "0.14.2": the numbers major.minor.patch.
fun compare(a: String, b: String): Int {
    fun parts(v: String) = v.trim().removePrefix("v").split('.').map { p -> p.takeWhile(Char::isDigit).toIntOrNull() ?: 0 }
    val (x, y) = parts(a) to parts(b)
    for (i in 0 until maxOf(x.size, y.size)) {
        val c = (x.getOrElse(i) { 0 }).compareTo(y.getOrElse(i) { 0 })
        if (c != 0) return c
    }
    return 0
}

private fun sha256(file: File): String {
    val digest = MessageDigest.getInstance("SHA-256")
    file.inputStream().use { input ->
        val buf = ByteArray(256 * 1024)
        while (true) {
            val n = input.read(buf)
            if (n < 0) break
            digest.update(buf, 0, n)
        }
    }
    return digest.digest().joinToString("") { "%02x".format(it) }
}

/// The package installer's answer to an update.
class InstallResult : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
        val message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
        val confirm = androidx.core.content.IntentCompat.getParcelableExtra(intent, Intent.EXTRA_INTENT, Intent::class.java)
        (context.applicationContext as ChessPenguinApp).updates.onInstallResult(status, message, confirm)
    }
}
