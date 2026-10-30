package io.github.fennec.recorder

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import androidx.room.Room
import io.github.fennec.recorder.data.AppDatabase
import io.github.fennec.recorder.data.DesktopStore
import io.github.fennec.recorder.data.SettingsStore
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.net.FennecClient
import io.github.fennec.recorder.net.SendTarget
import io.github.fennec.recorder.record.Recorder
import io.github.fennec.recorder.sync.CleanupWorker
import io.github.fennec.recorder.sync.MediaStoreCopies
import io.github.fennec.recorder.sync.Sender
import io.github.fennec.recorder.sync.SharedCopies
import io.github.fennec.recorder.sync.recordingsDir
import io.github.fennec.recorder.sync.usbSidecar
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import java.io.File

/** Everything the app shares: storage, Fennec's projects, settings. */
class AppGraph(
    val context: Context,
    val db: AppDatabase,
    val desktop: DesktopStore,
    val settings: SettingsStore,
    val dir: File,
    /** Copies for USB transfer. */
    val copies: SharedCopies,
) {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    /** A transfer from a scanned code (tests connect to their own Fennec). */
    fun sender(
        deviceName: String,
        connect: (SendTarget) -> FennecClient = { FennecClient(it.address, it.pin) },
    ) = Sender(db.recordings(), dir, desktop, deviceName, connect)

    suspend fun cleanUp(now: Long) {
        val days = settings.settings.value.keepDays
        if (days <= 0) return
        for (r in db.recordings().deliveredBefore(now - days * 86_400_000L)) {
            File(dir, r.fileName).delete()
            db.recordings().delete(r.id)
        }
    }

    /** Recordings the app was killed in the middle of (recording or sending): keep what was written. */
    suspend fun recoverUnfinished() {
        db.recordings().stopSending()
        if (Recorder.state.value is Recorder.Status.Recording) return
        for (r in db.recordings().unfinished()) {
            val file = File(dir, r.fileName)
            if (!file.exists() || file.length() == 0L) {
                file.delete()
                db.recordings().delete(r.id)
                continue
            }
            db.recordings().update(Recorder.finished(r, file, r.durationMs))
        }
    }

    // ---- USB transfer ----

    /** Gives recordings not yet sent their copy for USB transfer (if that is on). */
    suspend fun exportForUsb() {
        if (!settings.settings.value.usbCopies) return
        for (r in db.recordings().needingUsbCopy()) {
            val file = File(dir, r.fileName)
            if (!file.exists() || r.sha256 == null) continue
            val (audio, meta) = copies.export(file, r.id, usbSidecar(r)) ?: continue
            db.recordings().update(r.copy(usbAudio = audio, usbMeta = meta))
        }
    }

    /** The title, project or template changed: the sidecar says so too. */
    suspend fun refreshUsbSidecar(id: String) {
        val r = db.recordings().get(id) ?: return
        if (r.usbAudio == null) return
        val meta = copies.replaceSidecar(r.usbMeta, r.id, usbSidecar(r))
        db.recordings().update(r.copy(usbMeta = meta))
    }

    /**
     * Copies Fennec took over USB (it deletes them after importing) mark
     * their recordings sent; copies of recordings Fennec has otherwise are
     * removed, and so are all copies when USB transfer is turned off.
     */
    suspend fun checkUsb(now: Long = System.currentTimeMillis()) {
        val keep = settings.settings.value.usbCopies
        for (r in db.recordings().withUsbCopy()) {
            val gone = r.usbAudio == null || !copies.exists(r.usbAudio)
            when {
                r.state.delivered || !keep -> dropCopies(r)
                gone && r.state != SyncState.RECORDING -> {
                    r.usbMeta?.let(copies::delete)
                    db.recordings().update(
                        r.copy(state = SyncState.USB, deliveredAt = r.deliveredAt ?: now, error = null, usbAudio = null, usbMeta = null),
                    )
                }
            }
        }
    }

    private suspend fun dropCopies(r: io.github.fennec.recorder.data.Recording) {
        r.usbAudio?.let(copies::delete)
        r.usbMeta?.let(copies::delete)
        db.recordings().update(r.copy(usbAudio = null, usbMeta = null))
    }

    /** Fennec has it, though the phone could not tell (copied by hand, say). */
    suspend fun markSent(id: String) {
        val r = db.recordings().get(id) ?: return
        dropCopies(r)
        db.recordings().get(id)?.let {
            db.recordings().update(it.copy(state = SyncState.USB, deliveredAt = System.currentTimeMillis(), error = null))
        }
    }

    /** It was marked sent by mistake: send it again, with the next scan or over USB. */
    suspend fun markNotSent(id: String) {
        val r = db.recordings().get(id) ?: return
        db.recordings().update(r.copy(state = SyncState.WAITING, deliveredAt = null, sentBytes = 0, error = null))
        exportForUsb()
    }

    suspend fun deleteRecording(id: String) {
        val r = db.recordings().get(id) ?: return
        r.usbAudio?.let(copies::delete)
        r.usbMeta?.let(copies::delete)
        File(dir, r.fileName).delete()
        db.recordings().delete(id)
    }
}

class FennecApp : Application() {
    lateinit var graph: AppGraph

    override fun onCreate() {
        super.onCreate()
        graph = build(this)
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_RECORDING, "Recording", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Shown while Fennec Recorder records"
            },
        )
        graph.scope.launch {
            graph.recoverUnfinished()
            graph.checkUsb()
            graph.exportForUsb()
        }
        CleanupWorker.schedule(this)
    }

    companion object {
        const val CHANNEL_RECORDING = "recording"

        fun graph(context: Context): AppGraph = (context.applicationContext as FennecApp).graph

        fun build(
            context: Context,
            inMemory: Boolean = false,
            copies: SharedCopies? = null,
            /** Removes the old pairing's Keystore key (tests have no Keystore). */
            deleteOldKey: () -> Unit = DesktopStore::deleteOldKey,
        ): AppGraph {
            val db = if (inMemory) {
                Room.inMemoryDatabaseBuilder(context, AppDatabase::class.java).allowMainThreadQueries().build()
            } else {
                Room.databaseBuilder(context, AppDatabase::class.java, "recordings.db")
                    .addMigrations(AppDatabase.MIGRATION_2_3).build()
            }
            val desktop = DesktopStore(context.getSharedPreferences("desktop", MODE_PRIVATE))
            desktop.forgetOldPairing(context, deleteOldKey)
            return AppGraph(
                context.applicationContext,
                db,
                desktop,
                SettingsStore(context.getSharedPreferences("settings", MODE_PRIVATE)),
                recordingsDir(context),
                copies ?: MediaStoreCopies(context.contentResolver),
            )
        }
    }
}
