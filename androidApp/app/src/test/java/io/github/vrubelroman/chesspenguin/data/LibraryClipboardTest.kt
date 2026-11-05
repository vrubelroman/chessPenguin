package io.github.vrubelroman.chesspenguin.data

import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class LibraryClipboardTest {
    @Test
    fun `first valid record is accepted even when a later record is malformed`() {
        assertTrue(firstPgnGameIsMeaningful(count = 2, tags = 1, nodes = 1))
        assertTrue(firstPgnGameIsMeaningful(count = 2, tags = 0, nodes = 2))
    }

    @Test
    fun `first malformed record is rejected even when a later record is valid`() {
        assertFalse(firstPgnGameIsMeaningful(count = 2, tags = 0, nodes = 1))
    }

    @Test
    fun `multiple empty records and zero records are rejected`() {
        assertFalse(firstPgnGameIsMeaningful(count = 3, tags = 0, nodes = 1))
        assertFalse(firstPgnGameIsMeaningful(count = 0, tags = 0, nodes = 0))
    }

    @Test
    fun `tag only first record is accepted by the shared addCopy rule`() {
        assertTrue(firstPgnGameIsMeaningful(count = 1, tags = 1, nodes = 1))
    }

    @Test
    fun `final digest lookup reuses an existing copy after concurrent parsing`() {
        val existing = DbEntry(
            id = "existing",
            name = "Clipboard",
            source = Source.Copy,
            location = "existing.pgn",
            fileName = "clipboard.pgn",
            sha256 = "same-digest",
        )
        val other = existing.copy(id = "other", sha256 = "other-digest")

        assertSame(existing, copyWithDigest(listOf(other, existing), "same-digest"))
        assertNull(copyWithDigest(listOf(other, existing), "new-digest"))
    }
}
