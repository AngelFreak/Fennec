package io.github.fennec.recorder.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.RadioButton
import androidx.compose.material3.RadioButtonDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.fennec.recorder.R
import io.github.fennec.recorder.data.AppSettings
import io.github.fennec.recorder.data.DesktopInfo
import io.github.fennec.recorder.data.Paired
import io.github.fennec.recorder.data.Project
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.data.Template
import io.github.fennec.recorder.net.PairingTarget
import io.github.fennec.recorder.ui.theme.Fennec
import io.github.fennec.recorder.ui.theme.PlexMono
import io.github.fennec.recorder.ui.theme.SourceSerif
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

// ---- Recording detail ----

data class Playback(val playing: Boolean = false, val positionMs: Long = 0)

@Composable
fun DetailScreen(
    r: Recording,
    info: DesktopInfo,
    pairedName: String?,
    playback: Playback,
    onBack: () -> Unit,
    onPlay: () -> Unit,
    onRename: (String) -> Unit,
    onProject: (Project?) -> Unit,
    onTemplate: (Template?) -> Unit,
    onRetry: () -> Unit,
    onDelete: () -> Unit,
    onMarkSent: () -> Unit = {},
    onMarkNotSent: () -> Unit = {},
) {
    val c = Fennec.colors
    var renaming by remember { mutableStateOf(false) }
    var deleting by remember { mutableStateOf(false) }
    val editable = r.deliveredAt == null && (r.state == SyncState.WAITING || r.state == SyncState.FAILED)
    Column(Modifier.fillMaxSize()) {
        Header("Recording", navigation = { HeaderIcon(R.drawable.ic_back, "Back to recordings", onBack) })
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 22.dp),
            verticalArrangement = Arrangement.spacedBy(18.dp),
        ) {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(r.title, fontFamily = SourceSerif, fontSize = 24.sp, fontWeight = FontWeight.Medium, color = c.text)
                Text(
                    SimpleDateFormat("d MMM HH:mm", Locale.ENGLISH).format(Date(r.recordedAt)) +
                        " · ${clock(r.durationMs)} · " + "%.1f MB".format(r.size / 1_048_576.0),
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).border(1.dp, c.border, RoundedCornerShape(10.dp))
                    .padding(horizontal = 14.dp, vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Box(
                    Modifier.size(44.dp).clip(CircleShape).background(c.ink).clickable(role = Role.Button, onClick = onPlay)
                        .semantics { contentDescription = if (playback.playing) "Pause" else "Play" },
                    contentAlignment = Alignment.Center,
                ) { FennecIcon(if (playback.playing) R.drawable.ic_pause else R.drawable.ic_play, c.inkFg, 16.dp) }
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    ProgressLine(
                        if (r.durationMs > 0) playback.positionMs.toFloat() / r.durationMs else 0f,
                        color = c.ink,
                    )
                    Row {
                        Mono(clock(playback.positionMs), 11)
                        Box(Modifier.weight(1f))
                        Mono(clock(r.durationMs), 11)
                    }
                }
            }
            FennecBox {
                DetailPicker(
                    "Project", r.projectName ?: "Unsorted", editable,
                    listOf<Pair<String, Project?>>("Unsorted" to null) + info.projects.map { it.name to it }, first = true,
                    onPick = onProject,
                )
                DetailPicker(
                    "Template", r.templateName ?: "Fennec's default", editable,
                    listOf<Pair<String, Template?>>("Fennec's default" to null) + info.templates.map { it.name to it },
                    onPick = onTemplate,
                )
            }
            when {
                r.state == SyncState.USB -> UsbNote(
                    "Fennec took it over a USB cable, or it was marked sent. Its status shows here once this phone reaches Fennec on the same Wi-Fi.",
                    "Mark as not sent", onMarkNotSent,
                )
                r.deliveredAt == null && r.state != SyncState.RECORDING && r.state != SyncState.SENDING -> UsbNote(
                    if (r.usbAudio != null) {
                        "A copy waits in Download/Fennec Recorder. Plug the phone into the computer with a USB cable and choose File transfer: Fennec imports it, and this recording is marked sent."
                    } else {
                        "Moved it to Fennec yourself? Mark it sent, so it is not sent again."
                    },
                    "Mark as sent to Fennec", onMarkSent,
                )
            }
            Column {
                SectionTitle("To Fennec")
                Steps(r, pairedName)
            }
            r.error?.takeIf { r.state == SyncState.FAILED }?.let {
                Text(it, color = c.error, fontSize = 13.sp, modifier = Modifier.testTag("error"))
            }
        }
        Row(Modifier.padding(20.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            SecondaryButton("Delete from phone", Modifier.weight(1f), color = c.error) { deleting = true }
            if (r.state == SyncState.FAILED || r.state == SyncState.WAITING) {
                SecondaryButton(if (r.state == SyncState.FAILED) "Try again" else "Send now", Modifier.weight(1f), onClick = onRetry)
            } else {
                SecondaryButton("Rename", Modifier.weight(1f)) { renaming = true }
            }
        }
    }
    if (renaming) {
        var text by remember { mutableStateOf(r.title) }
        AlertDialog(
            onDismissRequest = { renaming = false },
            title = { Text("Rename recording") },
            text = {
                OutlinedTextField(text, { text = it }, singleLine = true, colors = fieldColors())
            },
            confirmButton = {
                TextButton({ renaming = false; onRename(text) }) { Text("Rename", color = c.accentText) }
            },
            dismissButton = { TextButton({ renaming = false }) { Text("Cancel", color = c.muted) } },
            containerColor = c.surface,
        )
    }
    if (deleting) {
        AlertDialog(
            onDismissRequest = { deleting = false },
            title = { Text("Delete from this phone?") },
            text = {
                Text(
                    if (r.deliveredAt != null) {
                        "Fennec on your computer keeps its copy and the transcript."
                    } else {
                        "Fennec has not received it yet, so the recording will be gone."
                    },
                )
            },
            confirmButton = { TextButton({ deleting = false; onDelete() }) { Text("Delete", color = c.error) } },
            dismissButton = { TextButton({ deleting = false }) { Text("Cancel", color = c.muted) } },
            containerColor = c.surface,
        )
    }
}

@Composable
private fun UsbNote(text: String, action: String, onAction: () -> Unit) {
    val c = Fennec.colors
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(c.chrome)
            .border(1.dp, c.border, RoundedCornerShape(10.dp)).padding(horizontal = 14.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text("USB cable", fontSize = 13.sp, fontWeight = FontWeight.SemiBold, color = c.text)
        Text(text, fontSize = 13.sp, lineHeight = 19.sp, color = c.muted)
        TextButton(onAction, contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp)) {
            Text(action, color = c.accentText, fontWeight = FontWeight.Medium)
        }
    }
}

@Composable
private fun <T> DetailPicker(
    label: String,
    shown: String,
    editable: Boolean,
    options: List<Pair<String, T>>,
    first: Boolean = false,
    onPick: (T) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    Box {
        BoxRow(label, first = first, onClick = if (editable) ({ open = true }) else null) {
            Text(shown, style = MaterialTheme.typography.bodyMedium)
        }
        DropdownMenu(open, onDismissRequest = { open = false }) {
            for ((name, value) in options) {
                DropdownMenuItem(text = { Text(name) }, onClick = { open = false; onPick(value) })
            }
        }
    }
}

private enum class Step { DONE, CURRENT, TODO, FAILED }

@Composable
private fun Steps(r: Recording, pairedName: String?) {
    val computer = pairedName ?: "Fennec"
    val sent = r.state.delivered
    val steps = listOf(
        Triple("Saved on this phone", r.sha256?.let { "Checksum ready" } ?: "Recording", if (r.state == SyncState.RECORDING) Step.CURRENT else Step.DONE),
        Triple(
            when {
                r.state == SyncState.USB -> "Sent by USB"
                sent -> "Sent to $computer"
                else -> "Sending to $computer"
            },
            when {
                r.state == SyncState.USB -> "Fennec has it; it was removed from the transfer folder"
                r.state == SyncState.SENDING -> "%.1f of %.1f MB · resumes if the Wi-Fi drops".format(r.sentBytes / 1_048_576.0, r.size / 1_048_576.0)
                sent -> "Arrived complete"
                r.state == SyncState.FAILED && r.deliveredAt == null -> "Not sent"
                pairedName == null -> "Pair with Fennec to send it"
                else -> "Waits until this phone is on the same Wi-Fi as Fennec"
            },
            when {
                sent -> Step.DONE
                r.state == SyncState.SENDING -> Step.CURRENT
                r.state == SyncState.FAILED -> Step.FAILED
                else -> Step.TODO
            },
        ),
        Triple(
            "Transcribing",
            if (r.state == SyncState.USB) "Shows once this phone reaches Fennec on Wi-Fi" else "In the Files queue in Fennec",
            when (r.state) {
                SyncState.QUEUED, SyncState.TRANSCRIBING -> Step.CURRENT
                SyncState.DONE -> Step.DONE
                SyncState.FAILED -> if (r.deliveredAt != null) Step.FAILED else Step.TODO
                else -> Step.TODO
            },
        ),
        Triple(
            "Transcribed",
            "Open it in Fennec on your computer",
            if (r.state == SyncState.DONE) Step.DONE else Step.TODO,
        ),
    )
    steps.forEachIndexed { i, (label, note, step) -> StepRow(label, note, step, last = i == steps.lastIndex, r) }
}

@Composable
private fun StepRow(label: String, note: String, step: Step, last: Boolean, r: Recording) {
    val c = Fennec.colors
    Row(Modifier.fillMaxWidth().semantics(mergeDescendants = true) {}) {
        Column(Modifier.width(22.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Box(
                Modifier.size(22.dp).clip(CircleShape)
                    .background(
                        when (step) {
                            Step.DONE -> c.ok
                            Step.FAILED -> c.error
                            else -> c.surface
                        },
                    )
                    .border(
                        when (step) {
                            Step.CURRENT -> 2.dp
                            Step.TODO -> 1.5.dp
                            else -> 0.dp
                        },
                        if (step == Step.CURRENT) c.accent else c.dash, CircleShape,
                    ),
                contentAlignment = Alignment.Center,
            ) { if (step == Step.DONE) FennecIcon(R.drawable.ic_check, Color.White, 12.dp) }
            if (!last) {
                Box(Modifier.width(2.dp).height(34.dp).background(if (step == Step.DONE) c.ok else c.track))
            }
        }
        Column(Modifier.padding(start = 12.dp, bottom = 14.dp).weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(
                label, fontSize = 14.sp,
                fontWeight = if (step == Step.CURRENT) FontWeight.SemiBold else FontWeight.Medium,
                color = if (step == Step.TODO) c.muted else c.text,
            )
            Text(note, fontSize = 12.5.sp, color = c.muted)
            if (step == Step.CURRENT && r.state == SyncState.SENDING) ProgressLine(percent(r) / 100f, Modifier.padding(top = 4.dp))
        }
    }
}

@Composable
fun fieldColors() = OutlinedTextFieldDefaults.colors(
    focusedBorderColor = Fennec.colors.accent,
    unfocusedBorderColor = Fennec.colors.border,
    cursorColor = Fennec.colors.accent,
    focusedLabelColor = Fennec.colors.accentText,
)

// ---- Pairing ----

sealed interface PairUi {
    /** Pointing the camera at Fennec's QR code. */
    data object Scan : PairUi

    /** No camera: typing the address and code. */
    data class Manual(val error: String? = null) : PairUi

    /** Opened from a link: confirm before connecting. */
    data class Confirm(val target: PairingTarget) : PairUi

    /** Waiting for Allow in Fennec. `code` once known. */
    data class Waiting(val target: PairingTarget, val code: String?) : PairUi

    data class Paired(val name: String) : PairUi

    data class Failed(val message: String) : PairUi
}

@Composable
fun PairScreen(
    ui: PairUi,
    camera: @Composable () -> Unit,
    onManual: () -> Unit,
    onSubmitManual: (address: String, code: String) -> Unit,
    onConfirm: (PairingTarget) -> Unit,
    onRetry: () -> Unit,
    onClose: () -> Unit,
    /** Shown as the last welcome step, with Not now instead of Close. */
    welcome: Boolean = false,
) {
    val c = Fennec.colors
    Column(Modifier.fillMaxSize()) {
        if (welcome) {
            Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 20.dp), verticalAlignment = Alignment.CenterVertically) {
                StepDots(WELCOME_STEPS, 2)
            }
        } else {
            Header("Pair with Fennec", navigation = { HeaderIcon(R.drawable.ic_close, "Close", onClose) })
        }
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            when (ui) {
                PairUi.Scan -> {
                    if (welcome) {
                        Text("Pair with Fennec", fontFamily = SourceSerif, fontSize = 28.sp, fontWeight = FontWeight.Medium, color = c.text)
                    }
                    Text(
                        "On your computer, open Fennec, go to Settings, then Phone, and choose Pair a phone. Point the camera at the code.",
                        style = MaterialTheme.typography.bodyMedium, color = c.textSoft,
                    )
                    SameNetworkNote()
                    Box(Modifier.fillMaxWidth().height(300.dp), contentAlignment = Alignment.Center) { camera() }
                    TextButton(onManual, Modifier.align(Alignment.CenterHorizontally)) {
                        Text("No camera? Enter the code instead", color = c.accentText)
                    }
                }
                is PairUi.Manual -> {
                    var address by remember { mutableStateOf("") }
                    var code by remember { mutableStateOf("") }
                    Text(
                        "Fennec shows its address and an 8-digit code under Settings → Phone → Pair a phone.",
                        style = MaterialTheme.typography.bodyMedium, color = c.textSoft,
                    )
                    SameNetworkNote()
                    OutlinedTextField(
                        address, { address = it }, label = { Text("Address") }, singleLine = true,
                        placeholder = { Text("192.168.1.20:47130") }, colors = fieldColors(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                        modifier = Modifier.fillMaxWidth().testTag("address"),
                    )
                    OutlinedTextField(
                        code, { code = it }, label = { Text("Code") }, singleLine = true,
                        placeholder = { Text("7305 1148") }, colors = fieldColors(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        textStyle = androidx.compose.ui.text.TextStyle(fontFamily = PlexMono, fontSize = 18.sp),
                        modifier = Modifier.fillMaxWidth().testTag("code"),
                    )
                    ui.error?.let { Text(it, color = c.error, fontSize = 13.sp) }
                    PrimaryButton("Pair", Modifier.fillMaxWidth()) { onSubmitManual(address, code) }
                }
                is PairUi.Confirm -> {
                    val t = ui.target
                    CodeCard {
                        Text("Pair with ${t.name ?: "Fennec"}?", fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.text)
                        Mono(t.address, 13, c.textSoft)
                        Text(
                            "Only pair with a computer you are sitting at. Fennec will show a code to compare, and you press Allow there.",
                            fontSize = 13.sp, color = c.muted,
                        )
                    }
                    PrimaryButton("Pair", Modifier.fillMaxWidth()) { onConfirm(t) }
                }
                is PairUi.Waiting -> {
                    CodeCard {
                        Text("Check the code on your computer", fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = c.text)
                        Text(
                            ui.code ?: "···· ····", fontFamily = PlexMono, fontSize = 30.sp, fontWeight = FontWeight.Medium,
                            letterSpacing = 2.sp, color = c.text, modifier = Modifier.testTag("pair-code"),
                        )
                        Text(
                            "If Fennec shows the same code, press Allow there. This phone waits for it.",
                            fontSize = 13.sp, color = c.muted,
                        )
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            CircularProgressIndicator(Modifier.size(14.dp), color = c.accent, strokeWidth = 2.dp, trackColor = c.track)
                            Text("Waiting for Allow", fontSize = 12.5.sp, color = c.muted)
                        }
                    }
                }
                is PairUi.Paired -> {
                    CodeCard {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Box(Modifier.size(22.dp).clip(CircleShape).background(c.ok), contentAlignment = Alignment.Center) {
                                FennecIcon(R.drawable.ic_check, Color.White, 12.dp)
                            }
                            Text("Paired with ${ui.name}", fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.text)
                        }
                        Text(
                            "Recordings go to ${ui.name} whenever this phone is on the same Wi-Fi network and Fennec is open. Recorded elsewhere, they wait on the phone until then.",
                            fontSize = 13.sp, color = c.muted,
                        )
                    }
                    PrimaryButton("Start recording", Modifier.fillMaxWidth(), onClick = onClose)
                }
                is PairUi.Failed -> {
                    CodeCard {
                        Text("Not paired", fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.text)
                        Text(ui.message, fontSize = 13.sp, color = c.error, modifier = Modifier.testTag("pair-error"))
                    }
                    PrimaryButton("Try again", Modifier.fillMaxWidth(), onClick = onRetry)
                }
            }
        }
        if (welcome && ui !is PairUi.Paired) {
            Box(Modifier.padding(20.dp)) {
                SecondaryButton("Not now, record first", Modifier.fillMaxWidth(), onClick = onClose)
            }
        } else if (ui is PairUi.Waiting || ui is PairUi.Confirm) {
            Box(Modifier.padding(20.dp)) { SecondaryButton("Cancel", Modifier.fillMaxWidth(), onClick = onClose) }
        }
    }
}

@Composable
private fun CodeCard(content: @Composable () -> Unit) {
    val c = Fennec.colors
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(c.chrome)
            .border(1.dp, c.border, RoundedCornerShape(10.dp)).padding(horizontal = 16.dp, vertical = 14.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) { content() }
}

// ---- Settings ----

@Composable
fun SettingsScreen(
    paired: Paired?,
    settings: AppSettings,
    info: DesktopInfo,
    version: String,
    onBack: () -> Unit,
    onPair: () -> Unit,
    onUnpair: () -> Unit,
    onChange: (AppSettings) -> Unit,
    onProjects: () -> Unit,
    onTemplates: () -> Unit,
    onUsbCopies: (Boolean) -> Unit = { on -> onChange(settings.copy(usbCopies = on)) },
) {
    val c = Fennec.colors
    var unpairing by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize()) {
        Header("Settings", navigation = { HeaderIcon(R.drawable.ic_back, "Back", onBack) })
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            SectionTitle("Computer")
            FennecBox {
                if (paired != null) {
                    Row(
                        Modifier.fillMaxWidth().padding(14.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                            Text(paired.name, fontSize = 14.sp, fontWeight = FontWeight.Medium, color = c.text)
                            Mono(paired.address)
                        }
                        SecondaryButton("Unpair", color = c.error) { unpairing = true }
                    }
                } else {
                    Row(
                        Modifier.fillMaxWidth().padding(14.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text("Not paired", Modifier.weight(1f), fontSize = 14.sp, color = c.muted)
                        PrimaryButton("Pair with Fennec", onClick = onPair)
                    }
                }
            }
            SameNetworkNote(Modifier.padding(top = 8.dp))
            Gap(8.dp)
            SectionTitle("Filing in Fennec")
            FennecBox {
                NavRow(
                    "Projects",
                    when (info.projects.size) {
                        0 -> "None yet"
                        1 -> "1 project"
                        else -> "${info.projects.size} projects"
                    },
                    first = true, onClick = onProjects,
                )
                NavRow("Templates", "For new recordings: " + (info.template(settings.defaultTemplate)?.name ?: "Fennec's default"), onClick = onTemplates)
            }
            Gap(8.dp)
            SectionTitle("Sending")
            FennecBox {
                ToggleRow(
                    "Only on Wi-Fi without a data limit",
                    "Recordings wait for a network like your home Wi-Fi.",
                    settings.unmeteredOnly,
                ) { onChange(settings.copy(unmeteredOnly = it)) }
            }
            Gap(8.dp)
            SectionTitle("USB cable")
            FennecBox {
                ToggleRow(
                    "Keep a copy for USB transfer",
                    "Recordings not yet sent are also kept in Download/Fennec Recorder. Plug the phone into the computer and choose File transfer: Fennec imports them and removes the copies. Other apps on the phone can read that folder.",
                    settings.usbCopies,
                ) { onUsbCopies(it) }
            }
            Gap(8.dp)
            SectionTitle("Audio")
            FennecBox {
                Choice("Standard", "16 kHz, about 21 MB an hour. What Fennec transcribes.", !settings.highQuality, first = true) {
                    onChange(settings.copy(highQuality = false))
                }
                Choice("High", "48 kHz, about 58 MB an hour, for keeping the audio itself.", settings.highQuality) {
                    onChange(settings.copy(highQuality = true))
                }
            }
            Gap(8.dp)
            SectionTitle("After Fennec has transcribed a recording")
            FennecBox {
                listOf(7 to "Delete it from this phone after 7 days", 30 to "Delete it after 30 days", 0 to "Keep it")
                    .forEachIndexed { i, (days, label) ->
                        Choice(label, null, settings.keepDays == days, first = i == 0) { onChange(settings.copy(keepDays = days)) }
                    }
            }
            Gap(12.dp)
            Text(
                "Recordings go only from this phone to the computer you paired, over your own Wi-Fi network. Nothing passes through the internet.",
                style = MaterialTheme.typography.bodySmall,
            )
            Mono("Fennec Recorder $version", 11, c.faint)
        }
    }
    if (unpairing && paired != null) {
        AlertDialog(
            onDismissRequest = { unpairing = false },
            title = { Text("Unpair from ${paired.name}?") },
            text = { Text("Recordings stay on this phone. To send them, pair again.") },
            confirmButton = { TextButton({ unpairing = false; onUnpair() }) { Text("Unpair", color = c.error) } },
            dismissButton = { TextButton({ unpairing = false }) { Text("Cancel", color = c.muted) } },
            containerColor = c.surface,
        )
    }
}

@Composable
private fun NavRow(title: String, note: String, first: Boolean = false, onClick: () -> Unit) {
    val c = Fennec.colors
    if (!first) androidx.compose.material3.HorizontalDivider(color = c.divider)
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp).clickable(role = Role.Button, onClick = onClick).padding(14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, fontSize = 14.sp, fontWeight = FontWeight.Medium, color = c.text)
            Text(note, fontSize = 12.5.sp, color = c.muted)
        }
        FennecIcon(R.drawable.ic_chevron, c.muted, 16.dp)
    }
}

@Composable
private fun ToggleRow(title: String, note: String, on: Boolean, onToggle: (Boolean) -> Unit) {
    val c = Fennec.colors
    Row(
        Modifier.fillMaxWidth().clickable(role = Role.Switch) { onToggle(!on) }.padding(14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, fontSize = 14.sp, fontWeight = FontWeight.Medium, color = c.text)
            Text(note, fontSize = 12.5.sp, color = c.muted)
        }
        Switch(
            on, onToggle,
            colors = SwitchDefaults.colors(
                checkedTrackColor = c.accent, checkedThumbColor = Color.White,
                uncheckedTrackColor = c.track, uncheckedBorderColor = c.border, uncheckedThumbColor = c.faint,
            ),
        )
    }
}

@Composable
private fun Choice(title: String, note: String?, selected: Boolean, first: Boolean = false, onPick: () -> Unit) {
    val c = Fennec.colors
    if (!first) androidx.compose.material3.HorizontalDivider(color = c.divider)
    Row(
        Modifier.fillMaxWidth().heightIn(min = 52.dp).clickable(role = Role.RadioButton, onClick = onPick)
            .padding(horizontal = 14.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(selected, onPick, colors = RadioButtonDefaults.colors(selectedColor = c.accent, unselectedColor = c.faint))
        Column(Modifier.padding(start = 6.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, fontSize = 14.sp, fontWeight = FontWeight.Medium, color = c.text)
            note?.let { Text(it, fontSize = 12.5.sp, color = c.muted) }
        }
    }
}
