package io.github.fennec.recorder.sync

import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.RecordingDao
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.net.Announcement
import io.github.fennec.recorder.net.Api
import io.github.fennec.recorder.net.FennecClient
import io.github.fennec.recorder.net.RemoteStatus
import java.io.File
import java.io.RandomAccessFile

/** How a round of sending ended. */
enum class Outcome {
    /** Nothing left to send. */
    DONE,

    /** Fennec was not reachable; try again later. */
    RETRY,

    /** Fennec no longer knows this phone. */
    UNPAIRED,

    /** Another computer answered at Fennec's address. */
    WRONG_COMPUTER,
}

/**
 * Sends recordings that are waiting, one at a time: announce, then chunks
 * from wherever Fennec's copy ends, then complete. Safe to stop at any point
 * and run again.
 */
class Uploader(
    private val dao: RecordingDao,
    private val client: FennecClient,
    private val dir: File,
    private val now: () -> Long = System::currentTimeMillis,
    private val chunkSize: Int = CHUNK,
) {
    suspend fun sendAll(): Outcome {
        for (r in dao.toSend()) {
            val outcome = send(r)
            if (outcome != Outcome.DONE) return outcome
        }
        return Outcome.DONE
    }

    private fun Api.Unreachable.outcome() = if (wrongComputer) Outcome.WRONG_COMPUTER else Outcome.RETRY

    /** Sends one recording; DONE also when it failed for good (marked FAILED). */
    suspend fun send(start: Recording): Outcome {
        var r = start
        val file = File(dir, r.fileName)
        if (!file.exists()) return fail(r, "The recording's file is missing from this phone.")
        val sha = r.sha256 ?: return fail(r, "The recording was not saved completely.")
        val announcement = Announcement(
            r.id, r.title, r.recordedAt, r.durationMs, r.projectId, r.templateId, r.ext, file.length(), sha,
        )
        var offset = when (val a = client.announce(announcement)) {
            is Api.Ok -> {
                if (a.value.state != "receiving") return delivered(r, a.value)
                a.value.received
            }
            is Api.Refused -> return refused(r, a)
            is Api.Unreachable -> return a.outcome()
        }
        dao.setSent(r.id, offset)
        var resends = r.resends
        var conflicts = 0
        while (true) {
            if (offset < file.length()) {
                val bytes = read(file, offset, chunkSize)
                when (val c = client.chunk(r.id, offset, bytes)) {
                    is Api.Ok -> {
                        offset = c.value
                        dao.setSent(r.id, offset)
                    }
                    is Api.Refused -> {
                        // Fennec's copy ends elsewhere (a chunk got lost or was resent): go on from there.
                        if (c.code == "offset" && c.received != null && conflicts++ < 5) {
                            offset = c.received
                            continue
                        }
                        return refused(r, c)
                    }
                    is Api.Unreachable -> return c.outcome()
                }
                continue
            }
            when (val done = client.complete(r.id)) {
                is Api.Ok -> return delivered(r, done.value)
                is Api.Refused -> when {
                    done.code == "checksum" && resends < 1 -> {
                        resends++
                        r = r.copy(resends = resends)
                        dao.update(r.copy(state = SyncState.SENDING, sentBytes = 0))
                        offset = 0
                    }
                    done.code == "incomplete" && done.received != null && conflicts++ < 5 -> offset = done.received
                    else -> return refused(r, done)
                }
                is Api.Unreachable -> return done.outcome()
            }
        }
    }

    private suspend fun delivered(r: Recording, s: RemoteStatus): Outcome {
        val current = dao.get(r.id) ?: return Outcome.DONE
        dao.update(current.applying(s, now()))
        return Outcome.DONE
    }

    private suspend fun refused(r: Recording, a: Api.Refused): Outcome {
        if (a.status == 401) return Outcome.UNPAIRED
        return fail(r, a.message)
    }

    private suspend fun fail(r: Recording, message: String): Outcome {
        val current = dao.get(r.id) ?: return Outcome.DONE
        dao.update(current.copy(state = SyncState.FAILED, error = message))
        return Outcome.DONE
    }

    companion object {
        const val CHUNK = 1 shl 20

        fun read(file: File, offset: Long, max: Int): ByteArray = RandomAccessFile(file, "r").use { f ->
            val n = minOf(max.toLong(), f.length() - offset).toInt()
            ByteArray(n).also { f.seek(offset); f.readFully(it) }
        }
    }
}

/** The recording as Fennec reports it. */
fun Recording.applying(s: RemoteStatus, now: Long): Recording {
    val state = SyncState.fromRemote(s.state) ?: return this
    return copy(
        state = state,
        sentBytes = if (state.delivered || state == SyncState.FAILED) size else s.received,
        documentId = s.documentId ?: documentId,
        error = if (state == SyncState.FAILED) s.error ?: "Fennec could not transcribe it." else null,
        deliveredAt = deliveredAt ?: if (state.delivered) now else null,
    )
}
