package io.github.fennec.recorder

import androidx.test.core.app.ApplicationProvider
import io.github.fennec.recorder.data.Paired
import io.github.fennec.recorder.net.Api
import io.github.fennec.recorder.net.FennecClient
import io.github.fennec.recorder.sync.hostPort
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import okhttp3.HttpUrl.Companion.toHttpUrl
import java.net.InetAddress

/** Fennec on another network or address, and unpairing away from it. */
@RunWith(RobolectricTestRunner::class)
class MovedTest {
    private val fennec = FakeFennec()
    private val other = FakeFennec()
    private var announced = listOf<String>()
    private lateinit var app: AppGraph

    @Before
    fun setUp() {
        app = FennecApp.build(
            ApplicationProvider.getApplicationContext(), PlainCipher, inMemory = true, copies = FakeCopies(), schedule = {},
            find = { announced },
        )
    }

    @After
    fun stop() {
        fennec.server.close()
        other.server.close()
    }

    private fun pairAt(address: String) =
        app.pairing.save(Paired("fennec-desktop", "abcdefabcdef", address, fennec.pin, 7, fennec.secret))

    /** Fennec's own address, spelled differently from the one paired with. */
    private val newAddress get() = "127.0.0.1:${fennec.server.port}"

    @Test
    fun `an address no URL can hold is unreachable, not a crash`() = runBlocking {
        val r = FennecClient("fe80::1%wlan0:47130", fennec.pin, fennec.secret).info()
        assertTrue("$r", r is Api.Unreachable && !r.wrongComputer)
    }

    @Test
    fun `Fennec is found at its new address on another Wi-Fi`() = runBlocking {
        pairAt("fe80::1%wlan0:47130")
        // Another computer is announced first: its certificate rules it out.
        announced = listOf("127.0.0.1:${other.server.port}", newAddress)
        app.refreshInfo()
        assertEquals(newAddress, app.pairing.paired.value?.address)
        assertEquals(true, app.lastContact.value.reachable)
        assertEquals("Kundemøder", app.pairing.info.value.projects.single().name)
    }

    @Test
    fun `nothing announced leaves the pairing as it was`() = runBlocking {
        pairAt("127.0.0.1:1")
        app.refreshInfo()
        assertEquals("127.0.0.1:1", app.pairing.paired.value?.address)
        assertEquals(false, app.lastContact.value.reachable)
    }

    @Test
    fun `unpairing away from Fennec happens at once`() {
        pairAt("10.255.255.1:47130") // Never answers.
        val started = System.currentTimeMillis()
        app.unpair()
        assertNull(app.pairing.paired.value)
        assertTrue("took ${System.currentTimeMillis() - started} ms", System.currentTimeMillis() - started < 1_000)
    }

    @Test
    fun `unpairing still tells Fennec when it can be reached`() = runBlocking {
        pairAt(fennec.address)
        app.unpair()
        assertNull(app.pairing.paired.value)
        withTimeout(10_000) {
            while ("DELETE /v1/devices/self" !in synchronized(fennec) { fennec.requests.toList() }) delay(20)
        }
    }

    @Test
    fun `announced addresses become ones a URL can hold`() {
        assertEquals("192.168.1.20:47130", hostPort(InetAddress.getByName("192.168.1.20"), 47130))
        val v6 = hostPort(InetAddress.getByName("2001:db8::5"), 47130)!!
        assertEquals("2001:db8::5", "https://$v6/".toHttpUrl().host)
        assertNull("link-local", hostPort(InetAddress.getByName("fe80::1"), 47130))
        assertNull("link-local IPv4", hostPort(InetAddress.getByName("169.254.3.4"), 47130))
        assertNull("the phone itself", hostPort(InetAddress.getByName("127.0.0.1"), 47130))
    }
}
