package io.github.fennec.recorder.record

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.MediaRecorder
import android.os.IBinder
import android.os.SystemClock
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import io.github.fennec.recorder.FennecApp
import io.github.fennec.recorder.MainActivity
import io.github.fennec.recorder.R
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.SyncState
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.io.File
import java.security.MessageDigest
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.UUID

/** What to record: chosen on the Record screen. */
data class RecordRequest(
    val title: String,
    val projectId: Long?,
    val projectName: String?,
    val templateId: String?,
    val templateName: String?,
)

/**
 * Records to raw AAC (ADTS) in the app's files. ADTS needs no index written
 * at the end, so a recording cut short (the app killed, the battery dead) is
 * still a playable file up to where it stopped.
 */
object Recorder {
    sealed interface Status {
        data object Idle : Status

        data class Recording(
            val id: String,
            val title: String,
            val projectName: String?,
            val templateName: String?,
            val elapsedMs: Long,
            val paused: Boolean,
            val bytes: Long,
            /** Recent loudness, 0–1, oldest first. */
            val levels: List<Float>,
        ) : Status

        data class Failed(val message: String) : Status
    }

    private val _state = MutableStateFlow<Status>(Status.Idle)
    val state: StateFlow<Status> = _state

    internal fun set(s: Status) {
        _state.value = s
    }

    fun start(context: Context, r: RecordRequest) {
        val i = Intent(context, RecordingService::class.java).setAction(ACTION_START)
            .putExtra("title", r.title)
            .putExtra("project_id", r.projectId ?: -1L)
            .putExtra("project_name", r.projectName)
            .putExtra("template_id", r.templateId)
            .putExtra("template_name", r.templateName)
        ContextCompat.startForegroundService(context, i)
    }

    fun pause(context: Context) = send(context, ACTION_PAUSE)
    fun resume(context: Context) = send(context, ACTION_RESUME)
    fun stop(context: Context) = send(context, ACTION_STOP)

    private fun send(context: Context, action: String) {
        context.startService(Intent(context, RecordingService::class.java).setAction(action))
    }

    /** The default title: when it was recorded, in Danish like the documents. */
    fun defaultTitle(now: Date = Date()): String =
        "Optagelse " + SimpleDateFormat("d. MMM HH:mm", Locale.forLanguageTag("da")).format(now)

    /** A recording whose file is complete: size and checksum, ready to send. */
    fun finished(r: Recording, file: File, durationMs: Long): Recording = r.copy(
        state = SyncState.WAITING,
        size = file.length(),
        sha256 = sha256(file),
        // Cut short with no time recorded: about 6 kB a second at 48 kbit/s.
        durationMs = if (durationMs > 0) durationMs else file.length() * 1000 / 6_000,
    )

    fun sha256(file: File): String {
        val md = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buf = ByteArray(1 shl 16)
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                md.update(buf, 0, n)
            }
        }
        return md.digest().joinToString("") { "%02x".format(it) }
    }

    const val ACTION_START = "io.github.fennec.recorder.START"
    const val ACTION_PAUSE = "io.github.fennec.recorder.PAUSE"
    const val ACTION_RESUME = "io.github.fennec.recorder.RESUME"
    const val ACTION_STOP = "io.github.fennec.recorder.STOP"
}

class RecordingService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var recorder: MediaRecorder? = null
    private var current: Recording? = null
    private var file: File? = null
    private var ticker: Job? = null

    /** Active recording time before the last resume. */
    private var activeBefore = 0L
    private var resumedAt = 0L
    private var paused = false
    private val levels = ArrayDeque<Float>()

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            Recorder.ACTION_START -> if (recorder == null) start(intent) else Unit
            Recorder.ACTION_PAUSE -> pause()
            Recorder.ACTION_RESUME -> resume()
            Recorder.ACTION_STOP -> stop()
        }
        return START_NOT_STICKY
    }

    private fun elapsed() = activeBefore + if (paused) 0 else SystemClock.elapsedRealtime() - resumedAt

    private fun start(intent: Intent) {
        val app = FennecApp.graph(this)
        val id = UUID.randomUUID().toString()
        val r = Recording(
            id = id,
            title = intent.getStringExtra("title")?.takeIf { it.isNotBlank() } ?: Recorder.defaultTitle(),
            recordedAt = System.currentTimeMillis(),
            projectId = intent.getLongExtra("project_id", -1L).takeIf { it >= 0 },
            projectName = intent.getStringExtra("project_name"),
            templateId = intent.getStringExtra("template_id"),
            templateName = intent.getStringExtra("template_name"),
            fileName = "$id.aac",
        )
        val f = File(app.dir, r.fileName)
        val high = app.settings.settings.value.highQuality
        // Foreground first: Android stops a service that waits too long.
        ServiceCompat.startForeground(
            this, NOTIFICATION, notification(r.title, false),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
        )
        val mr = MediaRecorder(this)
        try {
            mr.setAudioSource(MediaRecorder.AudioSource.MIC)
            mr.setOutputFormat(MediaRecorder.OutputFormat.AAC_ADTS)
            mr.setAudioEncoder(MediaRecorder.AudioEncoder.AAC)
            mr.setAudioChannels(1)
            mr.setAudioSamplingRate(if (high) 48_000 else 16_000)
            mr.setAudioEncodingBitRate(if (high) 128_000 else 48_000)
            mr.setOutputFile(f.absolutePath)
            mr.prepare()
            mr.start()
        } catch (e: Exception) {
            mr.release()
            f.delete()
            Recorder.set(Recorder.Status.Failed("The microphone could not start: ${e.message ?: e.javaClass.simpleName}"))
            stopSelf()
            return
        }
        recorder = mr
        current = r
        file = f
        activeBefore = 0
        resumedAt = SystemClock.elapsedRealtime()
        paused = false
        levels.clear()
        app.scope.launch { app.db.recordings().put(r) }
        ticker = scope.launch {
            while (isActive) {
                val level = if (paused) 0f else (mr.maxAmplitude / 32767f).coerceIn(0f, 1f)
                levels.addLast(level)
                while (levels.size > 48) levels.removeFirst()
                publish()
                delay(100)
            }
        }
    }

    private fun publish() {
        val r = current ?: return
        Recorder.set(
            Recorder.Status.Recording(
                r.id, r.title, r.projectName, r.templateName, elapsed(), paused, file?.length() ?: 0, levels.toList(),
            ),
        )
    }

    private fun pause() {
        val mr = recorder ?: return
        if (paused) return
        mr.pause()
        activeBefore = elapsed()
        paused = true
        current?.let { notify(it.title, true) }
        publish()
    }

    private fun resume() {
        val mr = recorder ?: return
        if (!paused) return
        mr.resume()
        resumedAt = SystemClock.elapsedRealtime()
        paused = false
        current?.let { notify(it.title, false) }
        publish()
    }

    private fun stop() {
        val mr = recorder
        val r = current
        val f = file
        val duration = elapsed()
        ticker?.cancel()
        recorder = null
        current = null
        if (mr != null) {
            runCatching { mr.stop() }
            mr.release()
        }
        if (r != null && f != null) {
            val app = FennecApp.graph(this)
            app.scope.launch {
                if (f.exists() && f.length() > 0) {
                    app.db.recordings().update(Recorder.finished(r, f, duration))
                    app.exportForUsb()
                    app.sendSoon()
                } else {
                    f.delete()
                    app.db.recordings().delete(r.id)
                }
            }
        }
        Recorder.set(Recorder.Status.Idle)
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        if (recorder != null) stop()
        scope.cancel()
        super.onDestroy()
    }

    private fun notify(title: String, paused: Boolean) {
        getSystemService(android.app.NotificationManager::class.java).notify(NOTIFICATION, notification(title, paused))
    }

    private fun action(action: String, label: String): NotificationCompat.Action {
        val pi = PendingIntent.getService(
            this, action.hashCode(), Intent(this, RecordingService::class.java).setAction(action),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Action(0, label, pi)
    }

    private fun notification(title: String, paused: Boolean): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE,
        )
        return NotificationCompat.Builder(this, FennecApp.CHANNEL_RECORDING)
            .setSmallIcon(R.drawable.ic_logo_mark)
            .setContentTitle(if (paused) "Paused" else "Recording")
            .setContentText(title)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(open)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .addAction(if (paused) action(Recorder.ACTION_RESUME, "Resume") else action(Recorder.ACTION_PAUSE, "Pause"))
            .addAction(action(Recorder.ACTION_STOP, "Stop"))
            .build()
    }

    companion object {
        const val NOTIFICATION = 1
    }
}
