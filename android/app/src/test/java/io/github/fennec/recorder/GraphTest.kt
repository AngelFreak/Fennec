package io.github.fennec.recorder

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import androidx.room.Room
import io.github.fennec.recorder.data.AppDatabase
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.record.Recorder
import kotlinx.coroutines.flow.first
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

@RunWith(RobolectricTestRunner::class)
class GraphTest {
    private lateinit var app: AppGraph

    @Before
    fun setUp() {
        app = FennecApp.build(ApplicationProvider.getApplicationContext(), inMemory = true, copies = FakeCopies())
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
    fun `sent recordings leave the phone after the chosen days`() = runBlocking {
        app.settings.update { it.copy(keepDays = 30) }
        val day = 86_400_000L
        val now = 100 * day
        put(Recording(id = "old", title = "a", recordedAt = 1, fileName = "old.aac", state = SyncState.SENT, deliveredAt = now - 31 * day), 10)
        put(Recording(id = "new", title = "b", recordedAt = 1, fileName = "new.aac", state = SyncState.USB, deliveredAt = now - 2 * day), 10)
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
    fun `a recording left Sending when the app was killed is not sent`() = runBlocking {
        put(Recording(id = "half", title = "a", recordedAt = 1, fileName = "half.aac", state = SyncState.SENDING, sentBytes = 4), 10)
        app.recoverUnfinished()
        assertEquals(SyncState.WAITING, app.db.recordings().get("half")!!.state)
    }

    @Test
    fun `the old pairing is forgotten once, keeping Fennec's projects`() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        context.getSharedPreferences("pairing", Context.MODE_PRIVATE).edit()
            .putString("name", "fennec-desktop").putString("id", "abcdef").putString("address", "192.168.1.20:47130")
            .putString("pin", "pin").putLong("device_id", 7).putString("secret", "sealed")
            .putString("info", """{"projects":[{"id":3,"name":"Kundemøder"}],"templates":[],"default_template":null}""")
            .commit()
        // The graph made in setUp already ran without an old pairing: start as an upgrade does.
        context.getSharedPreferences("desktop", Context.MODE_PRIVATE).edit().clear().commit()
        var keyDeleted = 0
        val upgraded = FennecApp.build(context, inMemory = true, copies = FakeCopies(), deleteOldKey = { keyDeleted++ })
        assertEquals(1, keyDeleted)
        assertTrue(context.getSharedPreferences("pairing", Context.MODE_PRIVATE).all.isEmpty())
        assertEquals("Kundemøder", upgraded.desktop.info.value.projects.single().name)
        assertEquals("fennec-desktop", upgraded.desktop.name.value)
        FennecApp.build(context, inMemory = true, copies = FakeCopies(), deleteOldKey = { keyDeleted++ })
        assertEquals("only once", 1, keyDeleted)
    }

    @Test
    fun `recordings from before sending by QR code are migrated`() = runBlocking {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val path = context.getDatabasePath("migrate.db").apply { parentFile?.mkdirs(); delete() }
        android.database.sqlite.SQLiteDatabase.openOrCreateDatabase(path, null).use { db ->
            // Version 2 as schemas/.../2.json has it.
            db.execSQL(
                "CREATE TABLE IF NOT EXISTS `recordings` (`id` TEXT NOT NULL, `title` TEXT NOT NULL, `recordedAt` INTEGER NOT NULL, " +
                    "`durationMs` INTEGER NOT NULL, `projectId` INTEGER, `projectName` TEXT, `templateId` TEXT, `templateName` TEXT, " +
                    "`fileName` TEXT NOT NULL, `ext` TEXT NOT NULL, `size` INTEGER NOT NULL, `sha256` TEXT, `state` TEXT NOT NULL, " +
                    "`sentBytes` INTEGER NOT NULL, `documentId` INTEGER, `error` TEXT, `deliveredAt` INTEGER, `resends` INTEGER NOT NULL, " +
                    "`usbAudio` TEXT, `usbMeta` TEXT, PRIMARY KEY(`id`))",
            )
            db.execSQL("CREATE TABLE IF NOT EXISTS room_master_table (id INTEGER PRIMARY KEY,identity_hash TEXT)")
            db.execSQL("INSERT OR REPLACE INTO room_master_table (id,identity_hash) VALUES(42, 'bcd72fd2615b2ee01f923613538b2788')")
            for ((id, state, delivered) in listOf(
                Triple("w", "WAITING", null), Triple("s", "SENDING", null), Triple("q", "QUEUED", 5L),
                Triple("t", "TRANSCRIBING", 5L), Triple("d", "DONE", 5L), Triple("f", "FAILED", null),
                Triple("ft", "FAILED", 5L), Triple("u", "USB", 5L),
            )) {
                db.execSQL(
                    "INSERT INTO recordings (id, title, recordedAt, durationMs, fileName, ext, size, state, sentBytes, error, deliveredAt, resends) " +
                        "VALUES (?, 'x', 1, 1, 'x.aac', 'aac', 1, ?, 0, 'e', ?, 0)",
                    arrayOf<Any?>(id, state, delivered),
                )
            }
            db.version = 2
        }
        val db = Room.databaseBuilder(context, AppDatabase::class.java, "migrate.db")
            .addMigrations(AppDatabase.MIGRATION_2_3).allowMainThreadQueries().build()
        val states = db.recordings().observeAll().first().associate { it.id to it.state }
        assertEquals(
            mapOf(
                "w" to SyncState.WAITING, "s" to SyncState.WAITING, "q" to SyncState.SENT, "t" to SyncState.SENT,
                "d" to SyncState.SENT, "f" to SyncState.FAILED, "ft" to SyncState.SENT, "u" to SyncState.USB,
            ),
            states,
        )
        assertEquals("transcription failed in Fennec: sent all the same", null, db.recordings().get("ft")!!.error)
        db.close()
    }
}
