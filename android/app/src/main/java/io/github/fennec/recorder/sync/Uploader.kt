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

/** How sending a recording ended. */
enum class Outcome {
    /** Sent, or refused for good (marked FAILED): go on with the next. */
    DONE,

    /** Fennec could not be reached. */
    UNREACHABLE,

    /** Fennec ended the session (Stop, or its dialog closed). */
    STOPPED,

    /** Another computer answered at Fennec's address. */
    WRONG_COMPUTER,
}

/**
 * Sends recordings over a session from a scanned code, one at a time:
 * announce, then chunks from wherever Fennec's copy ends, then complete.
 * Safe to stop at any point; the next scan resumes.
 */
class Uploader(
    private val dao: RecordingDao,
    private val client: FennecClient,
    private val dir: File,
    private val now: () -> Long = System::currentTimeMillis,
    private val chunkSize: Int = CHUNK,
) {
    private fun Api.Unreachable.outcome() = if (wrongComputer) Outcome.WRONG_COMPUTER else Outcome.UNREACHABLE

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
        dao.update(
            current.copy(
                state = SyncState.SENT, sentBytes = current.size, documentId = s.documentId ?: current.documentId,
                error = null, deliveredAt = current.deliveredAt ?: now(),
            ),
        )
        return Outcome.DONE
    }

    private suspend fun refused(r: Recording, a: Api.Refused): Outcome {
        if (a.status == 401) return Outcome.STOPPED
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
