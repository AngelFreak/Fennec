package io.github.fennec.recorder

import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import io.github.fennec.recorder.data.AppDatabase
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.net.Api
import io.github.fennec.recorder.net.FennecClient
import io.github.fennec.recorder.record.Recorder
import io.github.fennec.recorder.sync.Outcome
import io.github.fennec.recorder.sync.Uploader
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.io.File

/** Sending recordings over a session: chunks, resuming, damage, Fennec stopping. */
@RunWith(RobolectricTestRunner::class)
class UploaderTest {
    @get:Rule val tmp = TemporaryFolder()
    private val fennec = FakeFennec()
    private lateinit var db: AppDatabase
    private lateinit var dir: File

    @Before
    fun setUp() {
        db = Room.inMemoryDatabaseBuilder(ApplicationProvider.getApplicationContext(), AppDatabase::class.java)
            .allowMainThreadQueries().build()
        dir = tmp.newFolder("recordings")
    }

    @After
    fun tearDown() {
        db.close()
        fennec.server.close()
    }

    private fun recording(id: String, bytes: Int): Pair<Recording, ByteArray> = runBlocking {
        val data = ByteArray(bytes) { (it * 31 % 251).toByte() }
        val f = File(dir, "$id.aac").apply { writeBytes(data) }
        val r = Recorder.finished(
            Recording(id = id, title = "Møde $id", recordedAt = 1_791_000_000_000, projectId = 3, fileName = f.name),
            f, 61_000,
        )
        db.recordings().put(r)
        r to data
    }

    private val session: FennecClient by lazy {
        runBlocking {
            val client = FennecClient(fennec.address, fennec.pin)
            client.withSession((client.hello(fennec.ticket, "Pixel 8") as Api.Ok).value.session)
        }
    }

    private fun uploader(chunk: Int = 100_000) = Uploader(db.recordings(), session, dir, { 5L }, chunk)

    private fun stored(id: String) = runBlocking { db.recordings().get(id)!! }

    /** Every recording not sent, until one does not end DONE. */
    private suspend fun Uploader.sendAll(): Outcome {
        for (r in db.recordings().toSend()) {
            val o = send(r)
            if (o != Outcome.DONE) return o
        }
        return Outcome.DONE
    }

    @Test
    fun `a recording goes in chunks and is sent once complete answers`() = runBlocking {
        val (_, data) = recording("aaaaaaaa-1", 250_000)
        assertEquals(Outcome.DONE, uploader().sendAll())
        val r = stored("aaaaaaaa-1")
        assertEquals(SyncState.SENT, r.state)
        assertEquals(250_000L, r.sentBytes)
        assertEquals(41L, r.documentId)
        assertEquals(5L, r.deliveredAt)
        assertArrayEquals(data, fennec.recordings["aaaaaaaa-1"]!!.data.toByteArray())
        val meta = fennec.recordings["aaaaaaaa-1"]!!.meta
        assertEquals("\"Møde aaaaaaaa-1\"", meta["title"].toString())
        assertEquals("3", meta["project_id"].toString())
        assertEquals("aac", meta["ext"].toString().trim('"'))
        assertEquals(3, fennec.requests.count { it.endsWith("/audio") })
    }

    @Test
    fun `a lost answer is resumed where Fennec's copy ends`() = runBlocking {
        val (_, data) = recording("bbbbbbbb-1", 250_000)
        fennec.loseChunkAnswers = 1
        assertEquals(Outcome.UNREACHABLE, uploader().sendAll())
        assertEquals(SyncState.SENDING, stored("bbbbbbbb-1").state)
        // Fennec stored the chunk whose answer was lost; the next round goes on after it.
        assertEquals(Outcome.DONE, uploader().sendAll())
        assertArrayEquals(data, fennec.recordings["bbbbbbbb-1"]!!.data.toByteArray())
        assertEquals(SyncState.SENT, stored("bbbbbbbb-1").state)
    }

    @Test
    fun `a damaged upload is sent again from the start, once`() = runBlocking {
        val (_, data) = recording("cccccccc-1", 150_000)
        fennec.damageNext = true
        assertEquals(Outcome.DONE, uploader().sendAll())
        assertEquals(SyncState.SENT, stored("cccccccc-1").state)
        assertEquals(1, stored("cccccccc-1").resends)
        assertArrayEquals(data, fennec.recordings["cccccccc-1"]!!.data.toByteArray())
    }

    @Test
    fun `Fennec ending the session stops sending and keeps the recording`() = runBlocking {
        recording("dddddddd-1", 1000)
        session.done()
        assertEquals(Outcome.STOPPED, uploader().sendAll())
        assertEquals(SyncState.WAITING, stored("dddddddd-1").state)
    }

    @Test
    fun `a missing file fails that recording and the others still go`() = runBlocking {
        val (r, _) = recording("eeeeeeee-1", 1000)
        recording("ffffffff-1", 1000)
        File(dir, r.fileName).delete()
        assertEquals(Outcome.DONE, uploader().sendAll())
        assertEquals(SyncState.FAILED, stored("eeeeeeee-1").state)
        assertEquals(SyncState.SENT, stored("ffffffff-1").state)
    }

    @Test
    fun `a recording Fennec already has is not sent twice`() = runBlocking {
        recording("gggggggg-1", 5000)
        uploader().sendAll()
        db.recordings().update(stored("gggggggg-1").copy(state = SyncState.WAITING))
        val chunks = fennec.requests.count { it.endsWith("/audio") }
        assertEquals(Outcome.DONE, uploader().sendAll())
        assertEquals(chunks, fennec.requests.count { it.endsWith("/audio") })
        assertEquals(SyncState.SENT, stored("gggggggg-1").state)
    }
}
