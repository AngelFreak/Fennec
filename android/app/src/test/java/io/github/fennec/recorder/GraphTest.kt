package io.github.fennec.recorder

import androidx.test.core.app.ApplicationProvider
import io.github.fennec.recorder.data.Paired
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.SecretCipher
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.record.Recorder
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.io.File

/** Stands in for the Android Keystore, which Robolectric does not have. */
object PlainCipher : SecretCipher {
    override fun seal(plain: String) = plain.reversed()
    override fun open(sealed: String) = sealed.reversed()
}

@RunWith(RobolectricTestRunner::class)
class GraphTest {
    private lateinit var app: AppGraph

    @Before
    fun setUp() {
        app = FennecApp.build(ApplicationProvider.getApplicationContext(), PlainCipher, inMemory = true)
    }

    private fun put(r: Recording, bytes: Int) = runBlocking {
        File(app.dir, r.fileName).writeBytes(ByteArray(bytes) { 1 })
        app.db.recordings().put(r)
    }

    @Test
    fun `a recording cut short by the app being killed is kept and sent`() = runBlocking {
        put(Recording(id = "killed-1", title = "Møde", recordedAt = 1, fileName = "killed-1.aac"), 60_000)
        put(Recording(id = "empty-1", title = "Tom", recordedAt = 2, fileName = "empty-1.aac"), 0)
        app.recoverUnfinished()
        val kept = app.db.recordings().get("killed-1")!!
        assertEquals(SyncState.WAITING, kept.state)
        assertEquals(60_000L, kept.size)
        assertEquals(Recorder.sha256(File(app.dir, "killed-1.aac")), kept.sha256)
        assertEquals("about 6 kB a second", 10_000L, kept.durationMs)
        assertNull("an empty file is dropped", app.db.recordings().get("empty-1"))
        assertFalse(File(app.dir, "empty-1.aac").exists())
    }

    @Test
    fun `transcribed recordings leave the phone after the chosen days`() = runBlocking {
        app.settings.update { it.copy(keepDays = 30) }
        val day = 86_400_000L
        val now = 100 * day
        put(Recording(id = "old", title = "a", recordedAt = 1, fileName = "old.aac", state = SyncState.DONE, deliveredAt = now - 31 * day), 10)
        put(Recording(id = "new", title = "b", recordedAt = 1, fileName = "new.aac", state = SyncState.DONE, deliveredAt = now - 2 * day), 10)
        put(Recording(id = "unsent", title = "c", recordedAt = 1, fileName = "unsent.aac", state = SyncState.WAITING), 10)
        app.cleanUp(now)
        assertNull(app.db.recordings().get("old"))
        assertFalse(File(app.dir, "old.aac").exists())
        assertNotNull(app.db.recordings().get("new"))
        assertNotNull(app.db.recordings().get("unsent"))

        app.settings.update { it.copy(keepDays = 0) }
        app.cleanUp(now + 100 * day)
        assertNotNull("Keep it keeps it", app.db.recordings().get("new"))
    }

    @Test
    fun `the pairing survives a restart and the secret is not stored as is`() {
        app.pairing.save(Paired("fennec-desktop", "abcdefabcdef", "10.0.2.2:47131", "pin", 7, "the-secret"))
        val again = FennecApp.build(ApplicationProvider.getApplicationContext(), PlainCipher, inMemory = true)
        assertEquals("the-secret", again.pairing.paired.value?.secret)
        val prefs = ApplicationProvider.getApplicationContext<android.content.Context>()
            .getSharedPreferences("pairing", android.content.Context.MODE_PRIVATE)
        assertEquals("terces-eht", prefs.getString("secret", null))
        again.unpairedByFennec()
        assertNull(FennecApp.build(ApplicationProvider.getApplicationContext(), PlainCipher, inMemory = true).pairing.paired.value)
    }
}
