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
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
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
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.fennec.recorder.R
import io.github.fennec.recorder.data.AppSettings
import io.github.fennec.recorder.data.DesktopInfo
import io.github.fennec.recorder.data.Project
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.data.Template
import io.github.fennec.recorder.ui.theme.Fennec
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
    /** The computer last sent to, if any. */
    computerName: String?,
    playback: Playback,
    onBack: () -> Unit,
    onPlay: () -> Unit,
    onRename: (String) -> Unit,
    onProject: (Project?) -> Unit,
    onTemplate: (Template?) -> Unit,
    onSend: () -> Unit,
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
                    hint = NOT_LOADED.takeIf { info.empty }, onPick = onProject,
                )
                DetailPicker(
                    "Template", r.templateName ?: "Fennec's default", editable,
                    listOf<Pair<String, Template?>>("Fennec's default" to null) + info.templates.map { it.name to it },
                    hint = NOT_LOADED.takeIf { info.empty }, onPick = onTemplate,
                )
            }
            when {
                r.state == SyncState.USB -> UsbNote(
                    "Fennec took it over a USB cable, or it was marked sent.",
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
                Steps(r, computerName)
            }
            r.error?.takeIf { r.state == SyncState.FAILED }?.let {
                Text(it, color = c.error, fontSize = 13.sp, modifier = Modifier.testTag("error"))
            }
        }
        Row(Modifier.padding(20.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            SecondaryButton("Delete from phone", Modifier.weight(1f), color = c.error) { deleting = true }
            if (r.state == SyncState.FAILED || r.state == SyncState.WAITING) {
                PrimaryButton("Send to Fennec", Modifier.weight(1f), onClick = onSend)
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
    hint: String? = null,
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
            hint?.let { PickerHint(it) }
        }
    }
}

private enum class Step { DONE, CURRENT, TODO, FAILED }

@Composable
private fun Steps(r: Recording, computerName: String?) {
    val computer = computerName ?: "Fennec"
    val steps = listOf(
        Triple("Saved on this phone", r.sha256?.let { "Checksum ready" } ?: "Recording", if (r.state == SyncState.RECORDING) Step.CURRENT else Step.DONE),
        Triple(
            when (r.state) {
                SyncState.USB -> "Sent by USB"
                SyncState.SENT -> "Sent to $computer"
                SyncState.SENDING -> "Sending to $computer"
                SyncState.FAILED -> "Not sent"
                else -> "Not sent yet"
            },
            when (r.state) {
                SyncState.USB -> "Fennec has it; it was removed from the transfer folder"
                SyncState.SENT -> "Fennec transcribes it in its Files queue"
                SyncState.SENDING -> "%.1f of %.1f MB".format(r.sentBytes / 1_048_576.0, r.size / 1_048_576.0)
                SyncState.FAILED -> "Sent again with the next code you scan"
                SyncState.RECORDING -> "Once it is saved"
                else -> "In Fennec, choose Receive from phone, then Send to Fennec here"
            },
            when (r.state) {
                SyncState.SENT, SyncState.USB -> Step.DONE
                SyncState.SENDING -> Step.CURRENT
                SyncState.FAILED -> Step.FAILED
                else -> Step.TODO
            },
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

// ---- Settings ----

@Composable
fun SettingsScreen(
    settings: AppSettings,
    info: DesktopInfo,
    /** The computer last sent to, if any. */
    computerName: String?,
    version: String,
    onBack: () -> Unit,
    onSend: () -> Unit,
    onChange: (AppSettings) -> Unit,
    onTemplates: () -> Unit,
    onUsbCopies: (Boolean) -> Unit = { on -> onChange(settings.copy(usbCopies = on)) },
) {
    val c = Fennec.colors
    Column(Modifier.fillMaxSize()) {
        Header("Settings", navigation = { HeaderIcon(R.drawable.ic_back, "Back", onBack) })
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            SectionTitle("Sending")
            FennecBox {
                Column(Modifier.fillMaxWidth().padding(14.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text(HOW_TO_SEND, fontSize = 13.5.sp, lineHeight = 19.sp, color = c.text)
                    computerName?.let { Text("Last sent to $it.", fontSize = 12.5.sp, color = c.muted) }
                    PrimaryButton("Send to Fennec", onClick = onSend)
                }
            }
            SameNetworkNote(Modifier.padding(top = 8.dp))
            Gap(8.dp)
            SectionTitle("Filing in Fennec")
            FennecBox {
                NavRow(
                    "Templates",
                    if (info.templates.isEmpty()) {
                        "Shown once you have sent to Fennec"
                    } else {
                        "For new recordings: " + (info.template(settings.defaultTemplate)?.name ?: "Fennec's default")
                    },
                    first = true, onClick = onTemplates,
                )
            }
            Text(
                "Projects and templates are made in Fennec on your computer. Each scan brings the current ones to this phone.",
                style = MaterialTheme.typography.bodySmall,
            )
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
            SectionTitle("After a recording is sent to Fennec")
            FennecBox {
                listOf(7 to "Delete it from this phone after 7 days", 30 to "Delete it after 30 days", 0 to "Keep it")
                    .forEachIndexed { i, (days, label) ->
                        Choice(label, null, settings.keepDays == days, first = i == 0) { onChange(settings.copy(keepDays = days)) }
                    }
            }
            Gap(12.dp)
            Text(
                "Recordings go only from this phone to the computer whose code you scan, over your own Wi-Fi network. Nothing passes through the internet.",
                style = MaterialTheme.typography.bodySmall,
            )
            Mono("Fennec Recorder $version", 11, c.faint)
        }
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
