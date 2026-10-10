package io.github.fennec.recorder

import io.github.fennec.recorder.net.PairingCode
import io.github.fennec.recorder.net.PairingTarget
import io.github.fennec.recorder.net.PairingUri
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PairingTest {
    private val pin = "ODl9kE1pZ0yJ4o3b8zq5H2wTn7cVfR6sXuAaQeLgMtY"

    @Test
    fun `the code matches the one Fennec shows`() {
        // The same value is checked in Fennec's src/sync/pairing.rs.
        assertEquals("8428 0745", PairingCode.of(pin, "73051148", "ui-test-nonce-0123456789"))
    }

    @Test
    fun `nonces are long enough and use only characters Fennec accepts`() {
        val n = PairingCode.nonce()
        assertTrue(n.length in 16..128)
        assertTrue(n.all { it.isLetterOrDigit() || it == '-' || it == '_' })
    }

    @Test
    fun `Fennec's QR code is read with every part`() {
        val t = PairingUri.parse("fennec://pair?h=192.168.1.20:47130&pin=$pin&t=73051148&n=Ane%27s%20laptop")
        assertEquals(PairingTarget("192.168.1.20:47130", "73051148", pin, "Ane's laptop"), t)
    }

    @Test
    fun `other QR codes and damaged ones are ignored`() {
        assertNull(PairingUri.parse("https://example.com/pair?h=1.2.3.4:1&pin=$pin&t=12345678"))
        assertNull(PairingUri.parse("fennec://other?h=1.2.3.4:1&pin=$pin&t=12345678"))
        assertNull(PairingUri.parse("fennec://pair?h=1.2.3.4:1&pin=short&t=12345678"))
        assertNull(PairingUri.parse("fennec://pair?h=1.2.3.4:1&pin=$pin&t=1234"))
        assertNull(PairingUri.parse("fennec://pair?h=1.2.3.4&pin=$pin&t=12345678"))
        assertNull(PairingUri.parse("not a uri at all"))
    }

    @Test
    fun `a typed address gets the default port and the code may have spaces`() {
        assertEquals(PairingTarget("192.168.1.20:47130", "73051148", null, null), PairingUri.manual(" 192.168.1.20 ", "7305 1148"))
        assertEquals("10.0.2.2:5000", PairingUri.manual("10.0.2.2:5000", "73051148")?.address)
        assertNull(PairingUri.manual("", "73051148"))
        assertNull(PairingUri.manual("192.168.1.20", "7305"))
        assertNull(PairingUri.manual("192.168.1.20:99999", "73051148"))
    }
}
