package io.github.fennec.recorder.ui

import android.app.Application
import android.os.Build
import android.provider.Settings
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import io.github.fennec.recorder.FennecApp
import io.github.fennec.recorder.data.Project
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.Template
import io.github.fennec.recorder.net.ScannedCode
import io.github.fennec.recorder.net.SendUri
import io.github.fennec.recorder.sync.SendUi
import io.github.fennec.recorder.sync.Sender
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

sealed interface Screen {
    /** First run: 0 what the app does, 1 the microphone, 2 how sending works. */
    data class Welcome(val step: Int) : Screen
    data object Record : Screen
    data object Recordings : Screen
    data class Detail(val id: String) : Screen
    data object Send : Screen
    data object Settings : Screen
    data object Templates : Screen
}

class MainViewModel(application: Application) : AndroidViewModel(application) {
    val app = FennecApp.graph(application)
    val screen = MutableStateFlow<Screen>(
        if (app.settings.settings.value.welcomed) Screen.Record else Screen.Welcome(0),
    )

    val next = MutableStateFlow(NextRecording())
    val recordings = app.db.recordings().observeAll()
        .stateIn(viewModelScope, SharingStarted.Eagerly, emptyList())

    private val sender = app.sender(deviceName())
    val send = sender.ui
    private var sending: Job? = null

    /** Where closing the Send screen returns to. */
    private var beforeSend: Screen = Screen.Record

    fun go(s: Screen) {
        if (s == Screen.Send && screen.value != Screen.Send) {
            beforeSend = screen.value
            if (!sender.busy) sender.show(SendUi.Scan)
        }
        screen.value = s
    }

    /** Back: Detail and Settings return to the tab below; Send stops and closes. */
    fun back(): Boolean {
        when (val s = screen.value) {
            is Screen.Detail -> screen.value = Screen.Recordings
            Screen.Templates -> screen.value = Screen.Settings
            is Screen.Welcome -> if (s.step > 0) screen.value = Screen.Welcome(s.step - 1) else return false
            Screen.Send -> closeSend()
            Screen.Settings, Screen.Recordings -> screen.value = Screen.Record
            Screen.Record -> return false
        }
        return true
    }

    /** The welcome is over: straight to recording from now on. */
    fun finishWelcome() {
        app.settings.update { it.copy(welcomed = true) }
        screen.value = Screen.Record
    }

    fun setDefaultTemplate(id: String?) = app.settings.update { it.copy(defaultTemplate = id) }

    /** What the next recording is filed under, as Fennec had it at the last scan. */
    fun recordRequest(): io.github.fennec.recorder.record.RecordRequest {
        val n = next.value
        val info = app.desktop.info.value
        val project = n.project?.let { info.project(it.id) ?: it }
        val template = templateFor(n.template, project, info, app.settings.settings.value.defaultTemplate)
        return io.github.fennec.recorder.record.RecordRequest(n.title, project?.id, project?.name, template?.id, template?.name)
    }

    /** While a screen shows: notice copies Fennec took over a USB cable. */
    fun watch(): Job = viewModelScope.launch {
        while (isActive) {
            app.checkUsb()
            delay(3_000)
        }
    }

    // ---- sending ----

    /** A code from the camera: send at once. */
    fun scanned(text: String) {
        when (val code = SendUri.read(text)) {
            is ScannedCode.Send -> {
                sending?.cancel()
                sending = viewModelScope.launch { sender.send(code.target) }
            }
            ScannedCode.OldFennec -> sender.show(SendUi.Failed(Sender.OLD_CODE))
            ScannedCode.Other -> sender.show(SendUi.Failed(Sender.NOT_FENNEC))
        }
    }

    /** A `fennec://send` link from another camera app: the user scanned it on purpose, so send at once. */
    fun openLink(text: String) {
        go(Screen.Send)
        if (!sender.busy) scanned(text)
    }

    fun scanAgain() = sender.show(SendUi.Scan)

    /** Leaving the Send screen stops sending; the next scan resumes. */
    fun closeSend() {
        sending?.cancel()
        sending = null
        sender.show(SendUi.Scan)
        screen.value = beforeSend
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

    private fun edit(id: String, f: (Recording) -> Recording) {
        viewModelScope.launch {
            val r = app.db.recordings().get(id) ?: return@launch
            app.db.recordings().update(f(r))
            app.refreshUsbSidecar(id)
        }
    }

    fun delete(id: String) {
        screen.value = Screen.Recordings
        viewModelScope.launch { app.deleteRecording(id) }
    }

    fun markSent(id: String) {
        viewModelScope.launch { app.markSent(id) }
    }

    fun markNotSent(id: String) {
        viewModelScope.launch { app.markNotSent(id) }
    }

    /** USB copies turned on or off. */
    fun setUsbCopies(on: Boolean) {
        app.settings.update { it.copy(usbCopies = on) }
        viewModelScope.launch {
            app.checkUsb()
            app.exportForUsb()
        }
    }
}

/** Recordings the next scan sends. */
fun List<Recording>.unsentCount(): Int = count { it.state.unsent }
