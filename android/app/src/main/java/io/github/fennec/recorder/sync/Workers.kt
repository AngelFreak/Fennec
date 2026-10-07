package io.github.fennec.recorder.sync

import android.content.Context
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import io.github.fennec.recorder.FennecApp
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.net.Api
import java.io.File
import java.util.concurrent.TimeUnit

/** Sends waiting recordings whenever the phone has a network Fennec may be on. */
class UploadWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val app = FennecApp.graph(applicationContext)
        app.checkUsb()
        val paired = app.pairing.paired.value ?: return Result.success()
        var outcome = app.uploader(paired).sendAll()
        if (outcome == Outcome.RETRY) {
            // Fennec may have a new address on this network.
            val moved = findFennec(applicationContext, paired.id)
            if (moved != null && moved != paired.address) {
                app.pairing.moved(moved)
                outcome = app.uploader(app.pairing.paired.value!!).sendAll()
            }
        }
        app.lastContact.value = when (outcome) {
            Outcome.RETRY -> app.lastContact.value.copy(reachable = false)
            else -> app.lastContact.value.copy(reachable = true, at = System.currentTimeMillis())
        }
        return when (outcome) {
            Outcome.DONE -> {
                // Fennec has them now: their copies for USB can go.
                app.checkUsb()
                follow(applicationContext)
                Result.success()
            }
            Outcome.RETRY -> Result.retry()
            Outcome.UNPAIRED -> {
                app.unpairedByFennec()
                Result.success()
            }
            Outcome.WRONG_COMPUTER -> {
                app.lastContact.value = app.lastContact.value.copy(wrongComputer = true)
                Result.success()
            }
        }
    }

    companion object {
        private const val NAME = "upload"

        fun enqueue(context: Context, unmeteredOnly: Boolean) {
            val network = if (unmeteredOnly) NetworkType.UNMETERED else NetworkType.CONNECTED
            val req = OneTimeWorkRequestBuilder<UploadWorker>()
                .setConstraints(Constraints.Builder().setRequiredNetworkType(network).build())
                .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 30, TimeUnit.SECONDS)
                .build()
            WorkManager.getInstance(context).enqueueUniqueWork(NAME, ExistingWorkPolicy.REPLACE, req)
        }
    }
}

/** Follows recordings Fennec has until they are transcribed. */
class FollowWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val app = FennecApp.graph(applicationContext)
        return if (app.refreshStatuses()) Result.retry() else Result.success()
    }
}

/** Follows until every delivered recording is done or failed. */
fun follow(context: Context) {
    val req = OneTimeWorkRequestBuilder<FollowWorker>()
        .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
        .setBackoffCriteria(BackoffPolicy.LINEAR, 30, TimeUnit.SECONDS)
        .build()
    WorkManager.getInstance(context).enqueueUniqueWork("follow", ExistingWorkPolicy.KEEP, req)
}

/** Deletes recordings from the phone some days after Fennec transcribed them. */
class CleanupWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        FennecApp.graph(applicationContext).cleanUp(System.currentTimeMillis())
        return Result.success()
    }

    companion object {
        fun schedule(context: Context) {
            val req = PeriodicWorkRequestBuilder<CleanupWorker>(1, TimeUnit.DAYS).build()
            WorkManager.getInstance(context)
                .enqueueUniquePeriodicWork("cleanup", ExistingPeriodicWorkPolicy.KEEP, req)
        }
    }
}

/** True while something is still to follow. */
suspend fun io.github.fennec.recorder.AppGraph.refreshStatuses(): Boolean {
    val paired = pairing.paired.value ?: return false
    val open = db.recordings().toFollow()
    if (open.isEmpty()) return false
    when (val s = client(paired).statuses(open.map { it.id })) {
        is Api.Ok -> {
            val byId = s.value.associateBy { it.id }
            for (r in open) {
                val status = byId[r.id] ?: continue
                val updated = if (status.state == "unknown") {
                    // Taken over USB by a Fennec this phone is not paired with: nothing to follow.
                    if (r.state == SyncState.USB) r else r.copy(state = SyncState.FAILED, error = "Fennec no longer has this recording.")
                } else {
                    r.applying(status, System.currentTimeMillis())
                }
                if (updated != r) db.recordings().update(updated)
            }
            lastContact.value = lastContact.value.copy(reachable = true, at = System.currentTimeMillis())
        }
        is Api.Refused -> if (s.status == 401) unpairedByFennec()
        is Api.Unreachable -> lastContact.value = lastContact.value.copy(reachable = false)
    }
    return db.recordings().toFollow().isNotEmpty()
}

fun recordingsDir(context: Context) = File(context.filesDir, "recordings").apply { mkdirs() }
