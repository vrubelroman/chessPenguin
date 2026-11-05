package io.github.vrubelroman.chesspenguin

import io.github.vrubelroman.chesspenguin.data.DbEntry
import io.github.vrubelroman.chesspenguin.data.Source
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ClipboardIncomingTest {
    @Test
    fun `pasted existing copy ignores a previous empty filtered view`() {
        val shown = hashMapOf("copy" to emptyList<UInt>())
        val lastOpened = hashMapOf("copy" to ("not-found" to 0))

        resetPastedView(shown, lastOpened, "copy")

        assertFalse(shown.containsKey("copy"))
        assertFalse(lastOpened.containsKey("copy"))
    }

    @Test
    fun `uri and clipboard publish only their latest common request`() {
        val requests = LatestIncoming()
        val uri = requests.next()
        val clipboard = requests.next()

        assertFalse(requests.isLatest(uri))
        assertTrue(requests.isLatest(clipboard))

        val newerUri = requests.next()
        assertFalse(requests.isLatest(clipboard))
        assertTrue(requests.isLatest(newerUri))
    }

    @Test
    fun `stale request cannot publish after a newer request starts`() {
        val requests = LatestIncoming()
        val old = requests.next()
        val current = requests.next()
        var published: Long? = null

        assertFalse(requests.publish(old) { published = old })
        assertNull(published)
        assertTrue(requests.publish(current) { published = current })
        assertEquals(current, published)
    }

    @Test
    fun `blank clipboard snapshot does not invalidate uri import`() {
        val requests = LatestIncoming()
        val uri = requests.next(IncomingOrigin.Uri)

        assertFalse(requests.invalidateClipboard())
        assertTrue(requests.isLatest(uri))
    }

    @Test
    fun `clipboard snapshot invalidates active clipboard import`() {
        val requests = LatestIncoming()
        val clipboard = requests.next(IncomingOrigin.Clipboard)

        assertTrue(requests.invalidateClipboard())
        assertFalse(requests.isLatest(clipboard))
    }

    @Test
    fun `failed dismiss compareAndSet cannot clear a newer opening`() {
        val incoming = MutableStateFlow<Incoming?>(Incoming.Failed(IllegalStateException("old")))
        val expected = incoming.value
        incoming.value = Incoming.Opening

        assertFalse(incoming.compareAndSet(expected, null))
        assertEquals(Incoming.Opening, incoming.value)
    }

    @Test
    fun `stale collector cannot clear a newer incoming event`() {
        val entry = DbEntry(
            id = "copy",
            name = "Clipboard",
            source = Source.Copy,
            location = "copy.pgn",
            fileName = "clipboard.pgn",
        )
        val incoming = MutableStateFlow<Incoming?>(Incoming.Opened(entry))
        val expected = incoming.value
        incoming.value = Incoming.Opening

        assertFalse(incoming.compareAndSet(expected, null))
        assertEquals(Incoming.Opening, incoming.value)
    }
}
