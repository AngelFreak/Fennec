package io.github.fennec.recorder

import io.github.fennec.recorder.net.Api
import io.github.fennec.recorder.net.FennecClient
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Certificate pinning: the phone talks only to the Fennec it paired with. */
class ClientTest {
    private val fennec = FakeFennec()

    @After
    fun stop() = fennec.server.close()

    @Test
    fun `with the pin from the QR code the phone pairs`() = runBlocking {
        val r = FennecClient(fennec.address, fennec.pin).pair("12345678", "Pixel 8", "nonce-0123456789abcdef")
        assertTrue("$r", r is Api.Ok && r.value.secret == fennec.secret && r.value.name == "fennec-desktop")
    }

    @Test
    fun `another certificate is refused before anything is sent`() = runBlocking {
        val other = FakeFennec()
        val r = FennecClient(fennec.address, other.pin).pair("12345678", "Pixel 8", "nonce-0123456789abcdef")
        other.server.close()
        assertTrue("$r", r is Api.Unreachable && r.wrongComputer)
        assertTrue("no request reached Fennec", fennec.requests.isEmpty())
    }

    @Test
    fun `an address typed in learns the pin on first use`() = runBlocking {
        var seen: String? = null
        val r = FennecClient(fennec.address, null, onPin = { seen = it }).pair("12345678", "Pixel 8", "nonce-0123456789abcdef")
        assertTrue("$r", r is Api.Ok)
        assertEquals(fennec.pin, seen)
    }

    @Test
    fun `refusals carry Fennec's code and message`() = runBlocking {
        fennec.allowPairing = false
        val r = FennecClient(fennec.address, fennec.pin).pair("12345678", "Pixel 8", "nonce-0123456789abcdef")
        assertTrue("$r", r is Api.Refused && r.status == 403 && r.code == "denied" && r.message == "denied message")
    }

    @Test
    fun `nobody listening is unreachable, not a wrong computer`() = runBlocking {
        val address = fennec.address
        fennec.server.close()
        val r = FennecClient(address, fennec.pin, timeoutSeconds = 2).info()
        assertTrue("$r", r is Api.Unreachable && !r.wrongComputer)
    }

    @Test
    fun `projects and templates are read for the pickers`() = runBlocking {
        val r = FennecClient(fennec.address, fennec.pin, fennec.secret).info()
        assertTrue("$r", r is Api.Ok && r.value.projects.single().name == "Kundemøder" && r.value.templates.single().id == "notat")
    }
}
