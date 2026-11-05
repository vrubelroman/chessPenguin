package io.github.vrubelroman.chesspenguin.data

import android.content.SharedPreferences
import java.security.MessageDigest
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

internal const val CLIPBOARD_PGN_MAX_BYTES = 8 * 1024 * 1024
internal const val CLIPBOARD_PREFERENCES = "clipboard-pgn"
private const val PREFS_DIGESTS = "decided-sha256"

data class ClipboardOffer(
    val bytes: ByteArray,
    val games: Int,
    internal val digest: String,
    internal val generation: Long,
)

data class ClipboardAccept(
    val bytes: ByteArray,
    internal val digest: String,
    internal val generation: Long,
)

class ClipboardPgn(
    private val library: Library,
    private val scope: CoroutineScope,
    preferences: SharedPreferences,
    private val requestLock: Any,
    private val invalidateIncoming: () -> Unit,
) {
    private val gate = ClipboardPromptGate(
        decided = preferences.getStringSet(PREFS_DIGESTS, null).orEmpty(),
        onDecide = { digests -> preferences.edit().putStringSet(PREFS_DIGESTS, digests).apply() },
    )
    private val _offer = MutableStateFlow<ClipboardOffer?>(null)
    val offer: StateFlow<ClipboardOffer?> = _offer.asStateFlow()
    private var hasSnapshot = false
    private var lastSnapshot: String? = null

    fun check(text: String?) {
        val generation = synchronized(requestLock) {
            val changed = if (hasSnapshot && text == lastSnapshot) {
                false
            } else {
                hasSnapshot = true
                lastSnapshot = text
                true
            }
            if (!changed) return
            invalidateIncoming()
            gate.invalidate().also { _offer.value = null }
        }
        if (text.isNullOrBlank() || !clipboardPgnLengthMayFit(text)) return
        scope.launch {
            val bytes = clipboardUtf8(text)
            if (!clipboardPgnWithinLimit(bytes.size)) return@launch
            val digest = clipboardSha256(bytes)
            val games = try {
                library.countPgnGames(bytes)
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                null
            }
            if (games == null) return@launch
            val offer = ClipboardOffer(bytes, games, digest, generation)
            synchronized(requestLock) {
                if (gate.publish(offer)) {
                    _offer.value = offer
                }
            }
        }
    }

    fun accept(offer: ClipboardOffer): ClipboardAccept? = synchronized(requestLock) {
        gate.accept(offer)?.also { _offer.value = null }
    }

    fun dismiss(offer: ClipboardOffer) {
        synchronized(requestLock) {
            if (gate.dismiss(offer)) {
                _offer.value = null
            }
        }
    }

    fun canPublish(accept: ClipboardAccept): Boolean = synchronized(requestLock) { gate.canPublish(accept) }

    fun finish(accept: ClipboardAccept, success: Boolean): Boolean = synchronized(requestLock) {
        val finished = gate.finish(accept, success)
        if (finished && !success) {
            hasSnapshot = false
            lastSnapshot = null
        }
        finished
    }
}

internal class ClipboardPromptGate(
    decided: Set<String> = emptySet(),
    private val onDecide: (Set<String>) -> Unit = {},
) {
    private val decided = HashSet(decided)
    private var generation = 0L
    private var offer: ClipboardOffer? = null
    private var accepted: ClipboardAccept? = null

    @Synchronized
    fun invalidate(): Long {
        generation += 1
        offer = null
        accepted = null
        return generation
    }

    @Synchronized
    fun publish(offer: ClipboardOffer): Boolean {
        if (offer.generation != generation || offer.digest in decided || accepted != null) return false
        this.offer = offer
        return true
    }

    @Synchronized
    fun accept(offer: ClipboardOffer): ClipboardAccept? {
        if (this.offer != offer || accepted != null) return null
        val accept = ClipboardAccept(offer.bytes, offer.digest, offer.generation)
        accepted = accept
        this.offer = null
        return accept
    }

    @Synchronized
    fun dismiss(offer: ClipboardOffer): Boolean {
        if (this.offer != offer) return false
        this.offer = null
        decide(offer.digest)
        return true
    }

    @Synchronized
    fun canPublish(accept: ClipboardAccept): Boolean =
        accepted == accept && accept.generation == generation

    @Synchronized
    fun finish(accept: ClipboardAccept, success: Boolean): Boolean {
        if (accepted != accept || accept.generation != generation) return false
        accepted = null
        if (success) {
            decide(accept.digest)
        } else {
            invalidate()
        }
        return true
    }

    private fun decide(digest: String) {
        if (decided.add(digest)) onDecide(decided.toSet())
    }
}

internal fun clipboardPgnLengthMayFit(text: String): Boolean = text.length <= CLIPBOARD_PGN_MAX_BYTES

internal fun clipboardUtf8(text: String): ByteArray = text.toByteArray(Charsets.UTF_8)

internal fun clipboardSha256(bytes: ByteArray): String =
    MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

internal fun clipboardPgnWithinLimit(size: Int): Boolean = size <= CLIPBOARD_PGN_MAX_BYTES
