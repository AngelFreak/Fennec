package io.github.fennec.recorder

import io.github.fennec.recorder.net.Announcement
import io.github.fennec.recorder.net.Api
import io.github.fennec.recorder.net.FennecClient
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Protocol v2 from the phone's side: the pin, the ticket used once, the session. */
class ClientTest {
    private val fennec = FakeFennec()

    @After
    fun stop() = fennec.server.close()

    @Test
    fun `hello with the code's ticket opens a session and brings Fennec's projects`() = runBlocking {
        val r = FennecClient(fennec.address, fennec.pin).hello(fennec.ticket, "Pixel 8")
        assertTrue("$r", r is Api.Ok)
        val hello = (r as Api.Ok).value
        assertEquals(fennec.session, hello.session)
        assertEquals("fennec-desktop", hello.name)
        assertEquals(listOf("Pixel 8"), fennec.phones)
        assertEquals("Kundemøder", hello.info.projects.single().name)
        assertEquals(4, hello.info.projects.single().documents)
        assertEquals("Sagsnr.", hello.info.templates.single().fields.single().label)
        assertEquals("notat", hello.info.defaultTemplate)
        assertEquals(listOf("#C2410C", "#1D4ED8"), hello.info.projectColors)
    }

    @Test
    fun `a code works once`() = runBlocking {
        val client = FennecClient(fennec.address, fennec.pin)
        assertTrue(client.hello(fennec.ticket, "Pixel 8") is Api.Ok)
        val again = client.hello(fennec.ticket, "Another phone")
        assertTrue("$again", again is Api.Refused && again.status == 403 && again.code == "used" && again.message == "used message")
    }

    @Test
    fun `another certificate is refused before anything is sent`() = runBlocking {
        val other = FakeFennec()
        val r = FennecClient(fennec.address, other.pin).hello(fennec.ticket, "Pixel 8")
        other.server.close()
        assertTrue("$r", r is Api.Unreachable && r.wrongComputer)
        assertTrue("no request reached Fennec", fennec.requests.isEmpty())
    }

    @Test
    fun `calls carry the session, and without one Fennec refuses them`() = runBlocking {
        val a = Announcement("aaaa-1", "Møde", 1, 1000, null, null, "aac", 10, "ab")
        val none = FennecClient(fennec.address, fennec.pin).announce(a)
        assertTrue("$none", none is Api.Refused && none.status == 401 && none.code == "no_session")
        val hello = (FennecClient(fennec.address, fennec.pin).hello(fennec.ticket, "Pixel 8") as Api.Ok).value
        val client = FennecClient(fennec.address, fennec.pin).withSession(hello.session)
        assertTrue(client.announce(a) is Api.Ok)
        assertTrue(client.done() is Api.Ok)
        assertEquals(1, fennec.doneCalls)
        val after = client.announce(a)
        assertTrue("done ends the session: $after", after is Api.Refused && after.code == "no_session")
    }

    @Test
    fun `a pin mismatch is recognised when a later attempt failed too`() {
        val first = javax.net.ssl.SSLHandshakeException("handshake").apply { initCause(io.github.fennec.recorder.net.PinMismatch("x")) }
        val last = java.net.ConnectException("refused").apply { addSuppressed(first) }
        assertTrue(Api.Unreachable(last).wrongComputer)
        assertTrue(!Api.Unreachable(java.net.ConnectException("refused")).wrongComputer)
    }

    @Test
    fun `nobody listening is unreachable, not a wrong computer`() = runBlocking {
        val address = fennec.address
        fennec.server.close()
        val r = FennecClient(address, fennec.pin, timeoutSeconds = 2).hello(fennec.ticket, "Pixel 8")
        assertTrue("$r", r is Api.Unreachable && !r.wrongComputer)
    }

    @Test
    fun `an address no URL can hold is unreachable, not a crash`() = runBlocking {
        val r = FennecClient("fe80::1%wlan0:47130", fennec.pin).hello(fennec.ticket, "Pixel 8")
        assertTrue("$r", r is Api.Unreachable && !r.wrongComputer)
    }
}
