package io.github.fennec.recorder.sync

import android.content.Context
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import io.github.fennec.recorder.FennecApp
import java.io.File
import java.util.concurrent.TimeUnit

/** Deletes recordings from the phone some days after they were sent to Fennec. */
class CleanupWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        FennecApp.graph(applicationContext).cleanUp(System.currentTimeMillis())
        return Result.success()
    }

    companion object {
        fun schedule(context: Context) {
            val req = PeriodicWorkRequestBuilder<CleanupWorker>(1, TimeUnit.DAYS).build()
            val wm = WorkManager.getInstance(context)
            wm.enqueueUniquePeriodicWork("cleanup", ExistingPeriodicWorkPolicy.KEEP, req)
            // Sending and following ran in the background before sending by QR code.
            wm.cancelUniqueWork("upload")
            wm.cancelUniqueWork("follow")
        }
    }
}

fun recordingsDir(context: Context) = File(context.filesDir, "recordings").apply { mkdirs() }
