package io.github.fennec.recorder

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import androidx.room.Room
import io.github.fennec.recorder.data.AppDatabase
import io.github.fennec.recorder.data.KeystoreCipher
import io.github.fennec.recorder.data.Paired
import io.github.fennec.recorder.data.PairingStore
import io.github.fennec.recorder.data.SecretCipher
import io.github.fennec.recorder.data.SettingsStore
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.net.Api
import io.github.fennec.recorder.net.FennecClient
import io.github.fennec.recorder.record.Recorder
import io.github.fennec.recorder.sync.CleanupWorker
import io.github.fennec.recorder.sync.MediaStoreCopies
import io.github.fennec.recorder.sync.SharedCopies
import io.github.fennec.recorder.sync.usbSidecar
import io.github.fennec.recorder.sync.UploadWorker
import io.github.fennec.recorder.sync.Uploader
import io.github.fennec.recorder.sync.follow
import io.github.fennec.recorder.sync.recordingsDir
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import java.io.File

/** When the phone last heard from Fennec. */
data class Contact(
    val reachable: Boolean? = null,
    val at: Long? = null,
    /** Another computer answered at Fennec's address. */
    val wrongComputer: Boolean = false,
)

/** Everything the app shares: storage, the pairing, settings, the network. */
class AppGraph(
    val context: Context,
    val db: AppDatabase,
    val pairing: PairingStore,
    val settings: SettingsStore,
    val dir: File,
    /** Copies for USB transfer. */
    val copies: SharedCopies,
    /** Asks WorkManager to send waiting recordings (tests pass their own). */
    private val schedule: (AppGraph) -> Unit,
) {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    val lastContact = MutableStateFlow(Contact())

    fun client(p: Paired, timeoutSeconds: Long = 30) =
        FennecClient(p.address, p.pin, p.secret, timeoutSeconds = timeoutSeconds)

    fun uploader(p: Paired) = Uploader(db.recordings(), client(p), dir)

    /** Fennec answered "not paired": this phone was removed there. */
    fun unpairedByFennec() {
        pairing.forget()
        lastContact.value = Contact()
    }

    /** Unpairs here and tells Fennec, if it can be reached. */
    suspend fun unpair() {
        pairing.paired.value?.let { client(it).unpair() }
        pairing.forget()
        lastContact.value = Contact()
    }

    /** Reads Fennec's projects and templates for the pickers. */
    suspend fun refreshInfo() {
        val p = pairing.paired.value ?: return
        when (val info = client(p).info()) {
            is Api.Ok -> {
                pairing.saveInfo(info.value)
                lastContact.value = Contact(reachable = true, at = System.currentTimeMillis())
            }
            is Api.Refused -> if (info.status == 401) unpairedByFennec()
            is Api.Unreachable -> lastContact.value = lastContact.value.copy(
                reachable = false,
                wrongComputer = info.wrongComputer,
            )
        }
    }

    /** Adds a project (no [id]) or changes one in Fennec. Null when saved, else why not. */
    suspend fun saveProject(id: Long?, name: String, color: String, template: String?): String? {
        val p = pairing.paired.value ?: return "Pair with Fennec first."
        val c = client(p)
        val r = if (id == null) c.createProject(name.trim(), color, template) else c.updateProject(id, name.trim(), color, template)
        return when (r) {
            is Api.Ok -> {
                refreshInfo()
                null
            }
            is Api.Refused -> {
                if (r.status == 401) unpairedByFennec()
                r.message
            }
            is Api.Unreachable ->
                "Fennec could not be reached. Changing projects needs this phone on the same Wi-Fi network as Fennec, with Fennec open."
        }
    }

    suspend fun cleanUp(now: Long) {
        val days = settings.settings.value.keepDays
        if (days <= 0) return
        for (r in db.recordings().deliveredBefore(now - days * 86_400_000L)) {
            File(dir, r.fileName).delete()
            db.recordings().delete(r.id)
        }
    }

    /** Recordings the app was killed in the middle of: keep what was written. */
    suspend fun recoverUnfinished() {
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

    fun sendSoon() = schedule(this)

    // ---- USB transfer ----

    /** Gives recordings not yet sent their copy for USB transfer (if that is on). */
    suspend fun exportForUsb() {
        if (!settings.settings.value.usbCopies) return
        val deviceId = pairing.paired.value?.deviceId
        for (r in db.recordings().needingUsbCopy()) {
            val file = File(dir, r.fileName)
            if (!file.exists() || r.sha256 == null) continue
            val (audio, meta) = copies.export(file, r.id, usbSidecar(r, deviceId)) ?: continue
            db.recordings().update(r.copy(usbAudio = audio, usbMeta = meta))
        }
    }

    /** The title, project or template changed: the sidecar says so too. */
    suspend fun refreshUsbSidecar(id: String) {
        val r = db.recordings().get(id) ?: return
        if (r.usbAudio == null) return
        val meta = copies.replaceSidecar(r.usbMeta, r.id, usbSidecar(r, pairing.paired.value?.deviceId))
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

    /** It was marked sent by mistake: send it again, over Wi-Fi or USB. */
    suspend fun markNotSent(id: String) {
        val r = db.recordings().get(id) ?: return
        db.recordings().update(r.copy(state = SyncState.WAITING, deliveredAt = null, sentBytes = 0, error = null))
        exportForUsb()
        sendSoon()
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
        graph = build(this, KeystoreCipher())
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
            if (graph.db.recordings().toSend().isNotEmpty()) graph.sendSoon()
            if (graph.db.recordings().toFollow().isNotEmpty()) follow(this@FennecApp)
        }
        CleanupWorker.schedule(this)
    }

    companion object {
        const val CHANNEL_RECORDING = "recording"

        fun graph(context: Context): AppGraph = (context.applicationContext as FennecApp).graph

        fun build(
            context: Context,
            cipher: SecretCipher,
            inMemory: Boolean = false,
            copies: SharedCopies? = null,
            schedule: ((AppGraph) -> Unit)? = null,
        ): AppGraph {
            val db = if (inMemory) {
                Room.inMemoryDatabaseBuilder(context, AppDatabase::class.java).allowMainThreadQueries().build()
            } else {
                Room.databaseBuilder(context, AppDatabase::class.java, "recordings.db").build()
            }
            return AppGraph(
                context.applicationContext,
                db,
                PairingStore(context.getSharedPreferences("pairing", MODE_PRIVATE), cipher),
                SettingsStore(context.getSharedPreferences("settings", MODE_PRIVATE)),
                recordingsDir(context),
                copies ?: MediaStoreCopies(context.contentResolver),
                schedule ?: { g -> UploadWorker.enqueue(g.context, g.settings.settings.value.unmeteredOnly) },
            )
        }
    }
}

/** Waiting means not delivered yet: the project and template can still change. */
val SyncState.editable: Boolean get() = this == SyncState.WAITING || this == SyncState.FAILED
