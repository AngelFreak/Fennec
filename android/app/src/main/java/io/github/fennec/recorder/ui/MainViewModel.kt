package io.github.fennec.recorder.ui

import android.app.Application
import android.os.Build
import android.provider.Settings
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import io.github.fennec.recorder.FennecApp
import io.github.fennec.recorder.data.Paired
import io.github.fennec.recorder.data.Project
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.data.Template
import io.github.fennec.recorder.net.Api
import io.github.fennec.recorder.net.FennecClient
import io.github.fennec.recorder.net.PairingCode
import io.github.fennec.recorder.net.PairingTarget
import io.github.fennec.recorder.net.PairingUri
import io.github.fennec.recorder.sync.refreshStatuses
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.io.File

sealed interface Screen {
    data object Record : Screen
    data object Recordings : Screen
    data class Detail(val id: String) : Screen
    data object Pair : Screen
    data object Settings : Screen
}

class MainViewModel(application: Application) : AndroidViewModel(application) {
    val app = FennecApp.graph(application)
    val screen = MutableStateFlow<Screen>(if (app.pairing.paired.value == null) Screen.Pair else Screen.Record)
    val next = MutableStateFlow(NextRecording())
    val pair = MutableStateFlow<PairUi>(PairUi.Scan)
    val recordings = app.db.recordings().observeAll()
        .stateIn(viewModelScope, SharingStarted.Eagerly, emptyList())
    private var pairing: Job? = null

    fun go(s: Screen) {
        if (s == Screen.Pair && pair.value !is PairUi.Waiting) pair.value = PairUi.Scan
        screen.value = s
    }

    /** Back: Detail and Settings return to the tab below; Pair closes. */
    fun back(): Boolean {
        when (screen.value) {
            is Screen.Detail -> screen.value = Screen.Recordings
            Screen.Settings, Screen.Pair, Screen.Recordings -> {
                if (screen.value == Screen.Pair) cancelPairing()
                screen.value = Screen.Record
            }
            Screen.Record -> return false
        }
        return true
    }

    /** While a screen shows: read Fennec's projects and follow recordings it has. */
    fun watch(): Job = viewModelScope.launch {
        app.refreshInfo()
        while (isActive) {
            app.refreshStatuses()
            if (app.db.recordings().toSend().isNotEmpty() && app.pairing.paired.value != null) app.sendSoon()
            delay(3_000)
        }
    }

    // ---- pairing ----

    /** A QR code or link: from the camera, pair straight away; from a link, ask first. */
    fun pairWith(text: String, fromLink: Boolean): Boolean {
        val target = PairingUri.parse(text) ?: return false
        screen.value = Screen.Pair
        if (fromLink) pair.value = PairUi.Confirm(target) else startPairing(target)
        return true
    }

    fun pairManually(address: String, code: String) {
        val target = PairingUri.manual(address, code)
        if (target == null) {
            pair.value = PairUi.Manual("Enter the address as Fennec shows it (like 192.168.1.20:47130) and the 8-digit code.")
            return
        }
        startPairing(target)
    }

    fun startPairing(target: PairingTarget) {
        pairing?.cancel()
        val nonce = PairingCode.nonce()
        pair.value = PairUi.Waiting(target, target.pin?.let { PairingCode.of(it, target.token, nonce) })
        var seenPin: String? = target.pin
        pairing = viewModelScope.launch {
            val client = FennecClient(
                target.address, target.pin,
                onPin = { pin ->
                    seenPin = pin
                    val w = pair.value
                    if (w is PairUi.Waiting && w.code == null) pair.value = w.copy(code = PairingCode.of(pin, target.token, nonce))
                },
                timeoutSeconds = FennecClient.PAIR_TIMEOUT_SECONDS,
            )
            pair.value = when (val r = client.pair(target.token, deviceName(), nonce)) {
                is Api.Ok -> {
                    val pin = seenPin ?: return@launch
                    app.pairing.save(Paired(r.value.name, r.value.id, target.address, pin, r.value.deviceId, r.value.secret))
                    app.refreshInfo()
                    app.sendSoon()
                    PairUi.Paired(r.value.name)
                }
                is Api.Refused -> PairUi.Failed(r.message)
                is Api.Unreachable -> PairUi.Failed(
                    if (r.wrongComputer) {
                        "Another computer answered at ${target.address}. Scan the code again."
                    } else {
                        "Fennec could not be reached at ${target.address}. Check that this phone is on the same network and that receiving is on in Fennec."
                    },
                )
            }
        }
    }

    fun cancelPairing() {
        pairing?.cancel()
        pairing = null
        pair.value = PairUi.Scan
    }

    private fun deviceName(): String {
        val ctx = getApplication<Application>()
        return Settings.Global.getString(ctx.contentResolver, Settings.Global.DEVICE_NAME)?.takeIf { it.isNotBlank() }
            ?: Build.MODEL
    }

    // ---- recordings ----

    fun setNext(n: NextRecording) {
        next.value = n
    }

    fun rename(id: String, title: String) = edit(id) { it.copy(title = title.trim().ifEmpty { it.title }) }

    fun setProject(id: String, p: Project?) = edit(id) { it.copy(projectId = p?.id, projectName = p?.name) }

    fun setTemplate(id: String, t: Template?) = edit(id) { it.copy(templateId = t?.id, templateName = t?.name) }

    /** Sends again: a recording that failed, or one waiting for a better network. */
    fun retry(id: String) = edit(id) {
        if (it.deliveredAt == null) it.copy(state = SyncState.WAITING, error = null, sentBytes = 0) else it
    }.also { app.sendSoon() }

    private fun edit(id: String, f: (io.github.fennec.recorder.data.Recording) -> io.github.fennec.recorder.data.Recording) {
        viewModelScope.launch {
            val r = app.db.recordings().get(id) ?: return@launch
            app.db.recordings().update(f(r))
        }
    }

    fun delete(id: String) {
        screen.value = Screen.Recordings
        viewModelScope.launch {
            val r = app.db.recordings().get(id) ?: return@launch
            File(app.dir, r.fileName).delete()
            app.db.recordings().delete(id)
        }
    }

    fun unpair() {
        viewModelScope.launch { app.unpair() }
    }
}
