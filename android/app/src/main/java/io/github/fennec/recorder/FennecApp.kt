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

    fun sendSoon() = UploadWorker.enqueue(context)
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
            if (graph.db.recordings().toSend().isNotEmpty()) graph.sendSoon()
            if (graph.db.recordings().toFollow().isNotEmpty()) follow(this@FennecApp)
        }
        CleanupWorker.schedule(this)
    }

    companion object {
        const val CHANNEL_RECORDING = "recording"

        fun graph(context: Context): AppGraph = (context.applicationContext as FennecApp).graph

        fun build(context: Context, cipher: SecretCipher, inMemory: Boolean = false): AppGraph {
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
            )
        }
    }
}

/** Waiting means not delivered yet: the project and template can still change. */
val SyncState.editable: Boolean get() = this == SyncState.WAITING || this == SyncState.FAILED
