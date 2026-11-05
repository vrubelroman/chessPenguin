package io.github.vrubelroman.chesspenguin.data

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class ClipboardPromptGateTest {
    @Test
    fun `new offer is accepted once and same offer cannot be accepted twice`() {
        val gate = ClipboardPromptGate()
        val generation = gate.invalidate()
        val offer = offer("same", generation)
        assertTrue(gate.publish(offer))

        val accepted = gate.accept(offer)

        assertNotNull(accepted)
        assertNull(gate.accept(offer))
        assertTrue(gate.canPublish(accepted!!))
    }

    @Test
    fun `blank snapshot invalidates offer and pending publication`() {
        val gate = ClipboardPromptGate()
        val generation = gate.invalidate()
        val offer = offer("old", generation)
        assertTrue(gate.publish(offer))
        val accepted = gate.accept(offer)!!

        gate.invalidate()

        assertFalse(gate.canPublish(accepted))
        assertFalse(gate.publish(offer))
    }

    @Test
    fun `stale accept is ignored after snapshot changes`() {
        val gate = ClipboardPromptGate()
        val old = offer("old", gate.invalidate())
        assertTrue(gate.publish(old))
        gate.invalidate()
        val current = offer("current", gate.invalidate())

        assertNull(gate.accept(old))
        assertTrue(gate.publish(current))
        assertNotNull(gate.accept(current))
    }

    @Test
    fun `changed snapshot forbids old import publication`() {
        val gate = ClipboardPromptGate()
        val accepted = gate.accept(offer("old", gate.invalidate()).also { gate.publish(it) })!!

        gate.invalidate()

        assertFalse(gate.canPublish(accepted))
    }

    @Test
    fun `dismissed digest stays decided while changed text is allowed`() {
        val gate = ClipboardPromptGate()
        val dismissed = offer("dismissed", gate.invalidate())
        assertTrue(gate.publish(dismissed))
        assertTrue(gate.dismiss(dismissed))

        assertFalse(gate.publish(dismissed.copy(generation = gate.invalidate())))
        assertTrue(gate.publish(offer("changed", gate.invalidate())))
    }

    @Test
    fun `successful open persists digest only after entry and survives cold restoration`() {
        var persisted = emptySet<String>()
        val first = ClipboardPromptGate(onDecide = { persisted = it })
        val offer = offer("sha256-of-opened-text", first.invalidate())
        assertTrue(first.publish(offer))
        val accepted = first.accept(offer)!!
        assertTrue(persisted.isEmpty())

        assertTrue(first.finish(accepted, true))

        val restored = ClipboardPromptGate(decided = persisted)
        assertFalse(restored.publish(offer.copy(generation = restored.invalidate())))
        assertTrue(restored.publish(offer.copy(digest = "new-text", generation = restored.invalidate())))
    }

    @Test
    fun `failed import does not persist digest and retries after cold start`() {
        var persisted = emptySet<String>()
        val first = ClipboardPromptGate(onDecide = { persisted = it })
        val offer = offer("retry-digest", first.invalidate())
        assertTrue(first.publish(offer))
        val accepted = first.accept(offer)!!
        assertTrue(persisted.isEmpty())

        assertTrue(first.finish(accepted, false))

        val restored = ClipboardPromptGate(decided = persisted)
        assertTrue(restored.publish(offer.copy(generation = restored.invalidate())))
    }

    @Test
    fun `one UTF8 snapshot preserves BOM CRLF unicode and import bytes`() {
        val text = "\uFEFF[Event \"Партия 😀\"]\r\n1. e4 e5\r\n*"
        val bytes = clipboardUtf8(text)
        val gate = ClipboardPromptGate()
        val offer = ClipboardOffer(bytes, 1, clipboardSha256(bytes), gate.invalidate())
        assertTrue(gate.publish(offer))

        val accepted = gate.accept(offer)!!

        assertArrayEquals(text.toByteArray(Charsets.UTF_8), accepted.bytes)
        assertSame(bytes, accepted.bytes)
        assertNotEquals(
            clipboardSha256(bytes),
            clipboardSha256(clipboardUtf8(text.replace("\r\n", "\n"))),
        )
    }

    @Test
    fun `clipboard size limit includes eight MiB and rejects one byte more`() {
        assertTrue(clipboardPgnLengthMayFit("a".repeat(CLIPBOARD_PGN_MAX_BYTES)))
        assertFalse(clipboardPgnLengthMayFit("a".repeat(CLIPBOARD_PGN_MAX_BYTES + 1)))
        assertTrue(clipboardPgnWithinLimit(CLIPBOARD_PGN_MAX_BYTES - 1))
        assertTrue(clipboardPgnWithinLimit(CLIPBOARD_PGN_MAX_BYTES))
        assertFalse(clipboardPgnWithinLimit(CLIPBOARD_PGN_MAX_BYTES + 1))
    }

    private fun offer(digest: String, generation: Long, games: Int = 1): ClipboardOffer =
        ClipboardOffer(byteArrayOf(1), games, digest, generation)
}
