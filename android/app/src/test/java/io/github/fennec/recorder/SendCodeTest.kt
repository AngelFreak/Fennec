package io.github.fennec.recorder

import io.github.fennec.recorder.net.ScannedCode
import io.github.fennec.recorder.net.SendTarget
import io.github.fennec.recorder.net.SendUri
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SendCodeTest {
    private val pin = "ODl9kE1pZ0yJ4o3b8zq5H2wTn7cVfR6sXuAaQeLgMtY"
    private val ticket = "q8Zr3kVb1sXn0YtLm4Pw2A"

    @Test
    fun `Fennec's code is read with every part`() {
        assertEquals(
            SendTarget("192.168.1.20:47130", pin, ticket, "Ane's laptop"),
            SendUri.parse("fennec://send?h=192.168.1.20:47130&pin=$pin&t=$ticket&n=Ane%27s%20laptop"),
        )
        assertEquals("no name", null, SendUri.parse("fennec://send?h=10.0.2.2:47131&pin=$pin&t=$ticket")?.name)
        assertEquals("[2001:db8::5]:47130", SendUri.parse("fennec://send?h=[2001:db8::5]:47130&pin=$pin&t=$ticket")?.address)
    }

    @Test
    fun `a pairing code from an older Fennec is told apart`() {
        assertEquals(ScannedCode.OldFennec, SendUri.read("fennec://pair?h=1.2.3.4:47130&pin=$pin&t=73051148&n=pc"))
    }

    @Test
    fun `other codes and damaged ones are not sent to`() {
        assertEquals(ScannedCode.Other, SendUri.read("https://example.com/send?h=1.2.3.4:1&pin=$pin&t=$ticket"))
        assertEquals(ScannedCode.Other, SendUri.read("fennec://other?h=1.2.3.4:1&pin=$pin&t=$ticket"))
        assertNull(SendUri.parse("fennec://send?h=1.2.3.4:1&pin=short&t=$ticket"))
        assertNull(SendUri.parse("fennec://send?h=1.2.3.4:1&pin=$pin&t=short"))
        assertNull(SendUri.parse("fennec://send?h=1.2.3.4:1&pin=$pin"))
        assertNull(SendUri.parse("fennec://send?h=1.2.3.4&pin=$pin&t=$ticket"))
        assertNull(SendUri.parse("fennec://send?h=1.2.3.4:99999&pin=$pin&t=$ticket"))
        assertEquals("a damaged name is left out", null, SendUri.parse("fennec://send?h=1.2.3.4:1&pin=$pin&t=$ticket&n=%ZZ")?.name)
        assertEquals(ScannedCode.Other, SendUri.read("not a uri at all"))
    }
}
