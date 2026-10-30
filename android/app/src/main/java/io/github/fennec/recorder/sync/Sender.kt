package io.github.fennec.recorder.sync

import io.github.fennec.recorder.data.DesktopStore
import io.github.fennec.recorder.data.RecordingDao
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.net.Api
import io.github.fennec.recorder.net.FennecClient
import io.github.fennec.recorder.net.SendTarget
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.withContext
import java.io.File

/** Where sending to Fennec is, as the Send screen shows it. */
sealed interface SendUi {
    /** Pointing the camera at Fennec's code. */
    data object Scan : SendUi

    /** Saying hello with the code's ticket. */
    data class Connecting(val address: String, val name: String?) : SendUi

    /** Recordings going one at a time; [current] is on its way now. */
    data class Sending(val name: String, val ids: List<String>, val current: String?, val sent: Int) : SendUi

    data class Done(val name: String, val sent: Int, val failed: Int) : SendUi

    /** Stopped; [sent] recordings got there before it did. */
    data class Failed(val message: String, val sent: Int = 0) : SendUi
}

/**
 * One transfer from a scanned code: hello (which also brings Fennec's
 * projects and templates), then every recording not sent, failed ones too,
 * then done. Cancelling it (leaving the screen) leaves the rest not sent;
 * the next scan resumes where Fennec's copy ends.
 */
class Sender(
    private val dao: RecordingDao,
    private val dir: File,
    private val desktop: DesktopStore,
    private val deviceName: String,
    private val connect: (SendTarget) -> FennecClient = { FennecClient(it.address, it.pin) },
    private val now: () -> Long = System::currentTimeMillis,
    private val chunkSize: Int = Uploader.CHUNK,
) {
    private val _ui = MutableStateFlow<SendUi>(SendUi.Scan)
    val ui: StateFlow<SendUi> = _ui

    val busy: Boolean get() = _ui.value is SendUi.Connecting || _ui.value is SendUi.Sending

    fun show(ui: SendUi) {
        _ui.value = ui
    }

    suspend fun send(target: SendTarget) {
        try {
            run(target)
        } finally {
            withContext(NonCancellable) { dao.stopSending() }
        }
    }

    private suspend fun run(target: SendTarget) {
        _ui.value = SendUi.Connecting(target.address, target.name)
        val client = connect(target)
        val hello = when (val h = client.hello(target.ticket, deviceName)) {
            is Api.Ok -> h.value
            is Api.Refused -> return fail(helloRefused(h))
            is Api.Unreachable -> return fail(unreachable(target, h))
        }
        if (hello.session.isBlank()) return fail(OLD_FENNEC)
        desktop.save(hello.name, hello.info)
        val session = client.withSession(hello.session)
        val uploader = Uploader(dao, session, dir, now, chunkSize)
        val queue = dao.toSend()
        val ids = queue.map { it.id }
        var sent = 0
        var failed = 0
        for (id in ids) {
            _ui.value = SendUi.Sending(hello.name, ids, id, sent)
            // Read again: renamed, deleted or taken over USB since the list was made.
            val r = dao.get(id)?.takeIf { it.state.unsent } ?: continue
            if (r.state == SyncState.FAILED) dao.update(r.copy(state = SyncState.WAITING, error = null))
            when (uploader.send(r)) {
                Outcome.DONE -> if (dao.get(id)?.state == SyncState.SENT) sent++ else failed++
                Outcome.UNREACHABLE -> return fail(LOST, sent)
                Outcome.STOPPED -> return fail(STOPPED, sent)
                Outcome.WRONG_COMPUTER -> return fail(wrongComputer(target), sent)
            }
        }
        _ui.value = SendUi.Sending(hello.name, ids, null, sent)
        // Fennec closes the session either way; an error here changes nothing for the phone.
        session.done()
        _ui.value = SendUi.Done(hello.name, sent, failed)
    }

    private fun fail(message: String, sent: Int = 0) {
        _ui.value = SendUi.Failed(message, sent)
    }

    companion object {
        const val SAME_WIFI = "The phone and the computer must be on the same Wi-Fi network. No shared Wi-Fi? Use a USB cable."
        const val USED = "This code was already used. Show a new one in Fennec."
        const val EXPIRED = "This code has expired. Show a new one in Fennec."
        const val STOPPED = "Fennec stopped receiving. Scan a new code to send the rest."
        const val LOST = "The connection to Fennec was lost. Scan a new code to send the rest."
        const val OLD_FENNEC = "Fennec on the computer is too old for this app. Update Fennec on the computer, then choose Receive from phone."
        const val OLD_CODE = "This code is from an older version of Fennec. Update Fennec on the computer, then choose Receive from phone."
        const val NOT_FENNEC = "This is not a code from Fennec. In Fennec on your computer, choose Receive from phone and scan the code it shows."

        fun unreachable(t: SendTarget, u: Api.Unreachable) =
            if (u.wrongComputer) wrongComputer(t) else "Can't reach Fennec at ${t.address}. $SAME_WIFI"

        fun wrongComputer(t: SendTarget) =
            "Another computer answered at ${t.address}, not the Fennec that showed the code. Nothing was sent to it. Show a new code in Fennec and scan it again."

        fun helloRefused(r: Api.Refused) = when {
            r.code == "used" -> USED
            r.code == "no_ticket" -> EXPIRED
            r.code == "wrong_ticket" -> "This code was not accepted. Show a new one in Fennec."
            // Fennec before sending by QR code has no /v1/hello.
            r.status == 401 || r.status == 404 || r.status == 405 -> OLD_FENNEC
            else -> r.message
        }
    }
}
