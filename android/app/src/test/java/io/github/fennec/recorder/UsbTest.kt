package io.github.fennec.recorder

import androidx.test.core.app.ApplicationProvider
import io.github.fennec.recorder.data.Paired
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.sync.SharedCopies
import io.github.fennec.recorder.sync.usbSidecar
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.io.File

/** Shared storage as a map; "Fennec" removes entries the way it deletes over USB. */
class FakeCopies : SharedCopies {
    val files = linkedMapOf<String, String>()
    private var n = 0
    override fun export(audio: File, name: String, sidecar: String): Pair<String, String> {
        val a = "content://copies/${++n}"
        val m = "content://copies/${++n}"
        files[a] = "$name.${audio.extension}"
        files[m] = sidecar
        return a to m
    }
    override fun replaceSidecar(old: String?, name: String, sidecar: String): String {
        old?.let(files::remove)
        return "content://copies/${++n}".also { files[it] = sidecar }
    }
    override fun exists(uri: String) = uri in files
    override fun delete(uri: String) {
        files.remove(uri)
    }
}

@RunWith(RobolectricTestRunner::class)
class UsbTest {
    private val copies = FakeCopies()
    private lateinit var app: AppGraph

    @Before
    fun setUp() {
        app = FennecApp.build(ApplicationProvider.getApplicationContext(), PlainCipher, inMemory = true, copies = copies, schedule = {})
    }

    private fun saved(id: String, state: SyncState = SyncState.WAITING) = runBlocking {
        File(app.dir, "$id.aac").writeBytes(ByteArray(10) { 1 })
        app.db.recordings().put(
            Recording(id = id, title = "Møde $id", recordedAt = 5, durationMs = 1000, fileName = "$id.aac", size = 10, sha256 = "ab", state = state),
        )
    }

    private fun get(id: String) = runBlocking { app.db.recordings().get(id)!! }

    @Test
    fun `the sidecar is the one Fennec reads`() {
        val fixture = File(System.getProperty("fennec.fixtures.dir"), "usb/sidecar.json").readText().trim()
        val r = Recording(
            id = "5d1c2b3a-0e9f-4a8b-9c7d-6e5f4a3b2c1d", title = "Møde med Jensen", recordedAt = 1_791_295_320_000,
            durationMs = 1_083_000, projectId = 3, templateId = "moedereferat", fileName = "x.aac", ext = "aac", size = 4,
            sha256 = "03ac674216f3e15c761ee1a5e255f067953623c8b388b4459e13f978d7c846f4",
        )
        assertEquals("Fennec's tests/usb_import.rs reads the same file", fixture, usbSidecar(r, 7))
    }

    @Test
    fun `recordings not sent get a copy, and lose it when Fennec has them`() = runBlocking {
        app.pairing.save(Paired("pc", "id", "1.2.3.4:1", "pin", 9, "s"))
        saved("a")
        saved("b", SyncState.DONE)
        app.exportForUsb()
        val a = get("a")
        assertNotNull(a.usbAudio)
        assertTrue("the sidecar names the pairing", copies.files[a.usbMeta]!!.contains("\"device_id\":9"))
        assertNull("already transcribed: no copy", get("b").usbAudio)

        // Sent over Wi-Fi meanwhile: the copy goes, the state stays.
        app.db.recordings().update(a.copy(state = SyncState.QUEUED, deliveredAt = 1))
        app.checkUsb()
        assertNull(get("a").usbAudio)
        assertTrue(copies.files.isEmpty())
        assertEquals(SyncState.QUEUED, get("a").state)
    }

    @Test
    fun `a copy Fennec took over USB marks the recording sent`() = runBlocking {
        saved("a")
        app.exportForUsb()
        val r = get("a")
        app.checkUsb(now = 100)
        assertEquals("still there: nothing changes", SyncState.WAITING, get("a").state)

        copies.delete(r.usbAudio!!) // Fennec imported it and deleted the audio
        app.checkUsb(now = 100)
        val sent = get("a")
        assertEquals(SyncState.USB, sent.state)
        assertEquals(100L, sent.deliveredAt)
        assertNull(sent.usbAudio)
        assertTrue("the leftover sidecar is removed too", copies.files.isEmpty())
        assertTrue(app.db.recordings().toSend().isEmpty())
        assertEquals("followed once Fennec can be reached", listOf("a"), app.db.recordings().toFollow().map { it.id })
    }

    @Test
    fun `marked sent by hand, and back`() = runBlocking {
        saved("a")
        app.exportForUsb()
        app.markSent("a")
        assertEquals(SyncState.USB, get("a").state)
        assertTrue(copies.files.isEmpty())
        app.markNotSent("a")
        assertEquals(SyncState.WAITING, get("a").state)
        assertNull(get("a").deliveredAt)
        assertNotNull("a new copy for USB", get("a").usbAudio)
    }

    @Test
    fun `renaming rewrites the sidecar, deleting removes the copy`() = runBlocking {
        saved("a")
        app.exportForUsb()
        app.db.recordings().update(get("a").copy(title = "Nyt navn"))
        app.refreshUsbSidecar("a")
        assertTrue(copies.files[get("a").usbMeta]!!.contains("\"title\":\"Nyt navn\""))
        assertEquals(2, copies.files.size)
        app.deleteRecording("a")
        assertTrue(copies.files.isEmpty())
    }

    @Test
    fun `turning USB copies off removes them`() = runBlocking {
        saved("a")
        app.exportForUsb()
        app.settings.update { it.copy(usbCopies = false) }
        app.checkUsb()
        assertTrue(copies.files.isEmpty())
        assertEquals(SyncState.WAITING, get("a").state)
        app.exportForUsb()
        assertFalse(copies.files.isNotEmpty())
    }
}
