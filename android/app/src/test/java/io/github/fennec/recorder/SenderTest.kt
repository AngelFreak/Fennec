package io.github.fennec.recorder

import android.content.Context
import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import io.github.fennec.recorder.data.AppDatabase
import io.github.fennec.recorder.data.DesktopStore
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.net.FennecClient
import io.github.fennec.recorder.net.SendTarget
import io.github.fennec.recorder.record.Recorder
import io.github.fennec.recorder.sync.SendUi
import io.github.fennec.recorder.sync.Sender
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.io.File

/** One transfer from a scanned code, as the Send screen follows it. */
@RunWith(RobolectricTestRunner::class)
class SenderTest {
    @get:Rule val tmp = TemporaryFolder()
    private val fennec = FakeFennec()
    private lateinit var db: AppDatabase
    private lateinit var dir: File
    private lateinit var desktop: DesktopStore

    @Before
    fun setUp() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        db = Room.inMemoryDatabaseBuilder(context, AppDatabase::class.java).allowMainThreadQueries().build()
        dir = tmp.newFolder("recordings")
        desktop = DesktopStore(context.getSharedPreferences("sender-test", Context.MODE_PRIVATE))
    }

    @After
    fun tearDown() {
        db.close()
        fennec.server.close()
    }

    private fun recording(id: String, bytes: Int, at: Long, state: SyncState = SyncState.WAITING): ByteArray = runBlocking {
        val data = ByteArray(bytes) { (it * 7 % 251).toByte() }
        val f = File(dir, "$id.aac").apply { writeBytes(data) }
        val r = Recorder.finished(Recording(id = id, title = "Møde $id", recordedAt = at, fileName = f.name), f, 10_000)
        db.recordings().put(r.copy(state = state, error = if (state == SyncState.FAILED) "Fennec refused it." else null))
        data
    }

    private fun sender(connect: (SendTarget) -> FennecClient = { FennecClient(it.address, it.pin) }) =
        Sender(db.recordings(), dir, desktop, "Pixel 8", connect, { 9L }, chunkSize = 4_000)

    private fun state(id: String) = runBlocking { db.recordings().get(id)!!.state }

    @Test
    fun `a scan sends everything not sent, failed ones too, and says done`() = runBlocking {
        val a = recording("a", 10_000, 1)
        val b = recording("b", 3_000, 2, SyncState.FAILED)
        recording("c", 1_000, 3, SyncState.SENT)
        db.recordings().put(Recording(id = "d", title = "Optager", recordedAt = 4, fileName = "d.aac", state = SyncState.RECORDING))
        val s = sender()
        s.send(fennec.code())
        assertEquals(SendUi.Done("fennec-desktop", 2, 0), s.ui.value)
        assertEquals(SyncState.SENT, state("a"))
        assertEquals(SyncState.SENT, state("b"))
        assertEquals(null, db.recordings().get("b")!!.error)
        assertEquals(SyncState.RECORDING, state("d"))
        assertEquals(setOf("a", "b"), fennec.recordings.keys)
        assertArrayEquals(a, fennec.recordings["a"]!!.data.toByteArray())
        assertArrayEquals(b, fennec.recordings["b"]!!.data.toByteArray())
        assertEquals("the phone says it is done", 1, fennec.doneCalls)
        assertEquals(listOf("Pixel 8"), fennec.phones)
        // Fennec's projects and templates stay for the pickers.
        assertEquals("Kundemøder", desktop.info.value.projects.single().name)
        assertEquals("fennec-desktop", desktop.name.value)
    }

    @Test
    fun `with nothing to send a scan still loads Fennec's projects`() = runBlocking {
        val s = sender()
        s.send(fennec.code())
        assertEquals(SendUi.Done("fennec-desktop", 0, 0), s.ui.value)
        assertEquals("notat", desktop.info.value.defaultTemplate)
        assertEquals(1, fennec.doneCalls)
    }

    @Test
    fun `a used or expired code says what to do`() = runBlocking {
        recording("a", 1_000, 1)
        sender().send(fennec.code())
        recording("b", 1_000, 2)
        val s = sender()
        s.send(fennec.code())
        assertEquals(SendUi.Failed(Sender.USED), s.ui.value)
        fennec.receiving = false
        s.send(fennec.code())
        assertEquals(SendUi.Failed(Sender.EXPIRED), s.ui.value)
        assertEquals(SyncState.WAITING, state("b"))
    }

    @Test
    fun `Fennec stopping midway leaves the rest for the next code, which resumes`() = runBlocking {
        recording("a", 6_000, 1)
        val b = recording("b", 12_000, 2)
        recording("c", 2_000, 3)
        // a takes 2 chunks; Fennec stops after the first of b's 3.
        fennec.stopAfterChunks = 3
        val s = sender()
        s.send(fennec.code())
        assertEquals(SendUi.Failed(Sender.STOPPED, sent = 1), s.ui.value)
        assertEquals(SyncState.SENT, state("a"))
        assertEquals("not left as Sending", SyncState.WAITING, state("b"))
        assertEquals(SyncState.WAITING, state("c"))

        fennec.stopAfterChunks = Int.MAX_VALUE
        fennec.newCode()
        s.send(fennec.code())
        assertEquals(SendUi.Done("fennec-desktop", 2, 0), s.ui.value)
        assertArrayEquals(b, fennec.recordings["b"]!!.data.toByteArray())
        // Its first chunk, the one refused when Fennec stopped, then the two after the first: not from the start.
        assertEquals("b resumed after its first chunk", 4, fennec.requests.count { it.startsWith("PUT /v1/recordings/b/audio") })
    }

    @Test
    fun `a recording Fennec refuses fails, the others go, and the next scan tries it again`() = runBlocking {
        recording("a", 1_000, 1)
        recording("b", 1_000, 2)
        fennec.refuseTitle = "Møde a"
        val s = sender()
        s.send(fennec.code())
        assertEquals(SendUi.Done("fennec-desktop", 1, 1), s.ui.value)
        assertEquals(SyncState.FAILED, state("a"))
        assertEquals("too_long message", db.recordings().get("a")!!.error)
        assertEquals(SyncState.SENT, state("b"))

        fennec.refuseTitle = null
        fennec.newCode()
        s.send(fennec.code())
        assertEquals(SendUi.Done("fennec-desktop", 1, 0), s.ui.value)
        assertEquals(SyncState.SENT, state("a"))
    }

    @Test
    fun `nobody at the address says to use the same Wi-Fi or a cable`() = runBlocking {
        recording("a", 1_000, 1)
        val target = fennec.code()
        fennec.server.close()
        val s = sender { FennecClient(it.address, it.pin, timeoutSeconds = 2) }
        s.send(target)
        val failed = s.ui.value as SendUi.Failed
        assertEquals("Can't reach Fennec at ${target.address}. ${Sender.SAME_WIFI}", failed.message)
        assertEquals(SyncState.WAITING, state("a"))
    }

    @Test
    fun `another computer at the address gets nothing`() = runBlocking {
        recording("a", 1_000, 1)
        val other = FakeFennec()
        val s = sender()
        s.send(fennec.code().copy(pin = other.pin))
        other.server.close()
        assertEquals(SendUi.Failed(Sender.wrongComputer(fennec.code())), s.ui.value)
        assertTrue(fennec.requests.isEmpty())
    }

    @Test
    fun `leaving the screen stops sending and nothing stays Sending`() = runBlocking {
        recording("a", 40_000, 1)
        fennec.chunkDelayMs = 100
        val s = sender()
        val job = launch(Dispatchers.Default) { s.send(fennec.code()) }
        withTimeout(10_000) {
            while ((s.ui.value as? SendUi.Sending)?.current == null || state("a") != SyncState.SENDING) delay(20)
        }
        job.cancelAndJoin()
        assertEquals(SyncState.WAITING, state("a"))
        assertEquals(0, fennec.doneCalls)
    }
}
