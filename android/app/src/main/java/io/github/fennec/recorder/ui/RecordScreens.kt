package io.github.fennec.recorder.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.fennec.recorder.R
import io.github.fennec.recorder.data.DesktopInfo
import io.github.fennec.recorder.data.Project
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.data.Template
import io.github.fennec.recorder.record.Recorder
import io.github.fennec.recorder.ui.theme.Fennec
import io.github.fennec.recorder.ui.theme.PlexMono
import io.github.fennec.recorder.ui.theme.SourceSerif
import java.text.SimpleDateFormat
import java.util.Calendar
import java.util.Date
import java.util.Locale

/** What the Record screen is set to file the next recording under. */
data class NextRecording(val title: String = "", val project: Project? = null, val template: Template? = null)

@Composable
fun RecordScreen(
    status: Recorder.Status,
    next: NextRecording,
    info: DesktopInfo,
    /** Something waits in Settings: not paired, or Fennec not reachable. */
    attention: Boolean,
    phoneDefault: String?,
    onManageProjects: () -> Unit,
    onNext: (NextRecording) -> Unit,
    onRecord: () -> Unit,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onStop: () -> Unit,
    onSettings: () -> Unit,
) {
    when (status) {
        is Recorder.Status.Recording -> RecordingNow(status, onPause, onResume, onStop)
        else -> ReadyToRecord(
            next, info, attention, (status as? Recorder.Status.Failed)?.message, phoneDefault,
            onManageProjects, onNext, onRecord, onSettings,
        )
    }
}

@Composable
private fun ReadyToRecord(
    next: NextRecording,
    info: DesktopInfo,
    attention: Boolean,
    error: String?,
    phoneDefault: String?,
    onManageProjects: () -> Unit,
    onNext: (NextRecording) -> Unit,
    onRecord: () -> Unit,
    onSettings: () -> Unit,
) {
    val c = Fennec.colors
    Column(Modifier.fillMaxSize()) {
        Header("Fennec", navigation = {
            Box(Modifier.padding(start = 12.dp, end = 10.dp)) { FennecLogo(28.dp) }
        }) { HeaderIconWithDot(R.drawable.ic_settings, "Settings", attention, onSettings) }
        Column(Modifier.weight(1f).padding(horizontal = 20.dp, vertical = 16.dp)) {
            SectionTitle("New recording")
            FennecBox {
                BoxRow("Title", first = true) {
                    BasicTextField(
                        next.title,
                        { onNext(next.copy(title = it)) },
                        singleLine = true,
                        textStyle = TextStyle(fontFamily = SourceSerif, fontSize = 16.sp, color = c.text),
                        cursorBrush = SolidColor(c.accent),
                        modifier = Modifier.fillMaxWidth().testTag("title")
                            .semantics { contentDescription = "Title" },
                        decorationBox = { inner ->
                            if (next.title.isEmpty()) {
                                Text(Recorder.defaultTitle(), fontFamily = SourceSerif, fontSize = 16.sp, color = c.faint)
                            }
                            inner()
                        },
                    )
                }
                Picker(
                    "Project",
                    next.project?.name ?: "Unsorted",
                    next.project?.color,
                    listOf<Pair<String, Project?>>("Unsorted" to null) + info.projects.map { it.name to it },
                    manage = "Manage projects" to onManageProjects,
                ) { onNext(next.copy(project = it)) }
                Picker(
                    "Template",
                    next.template?.name ?: defaultTemplateLabel(next.project, info, phoneDefault),
                    null,
                    listOf<Pair<String, Template?>>("Default" to null) + info.templates.map { it.name to it },
                ) { onNext(next.copy(template = it)) }
            }
            Column(
                Modifier.weight(1f).fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                Text("00:00", fontFamily = PlexMono, fontSize = 60.sp, fontWeight = FontWeight.Medium, color = c.muted)
                Gap(14.dp)
                LevelBars(emptyList(), active = false, count = 24)
                error?.let {
                    Gap(10.dp)
                    Text(it, color = c.error, fontSize = 13.sp, textAlign = TextAlign.Center)
                }
            }
            Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
                RecordButton(recording = false, onClick = onRecord)
                Gap(10.dp)
                Text("Record", style = MaterialTheme.typography.labelLarge)
                Gap(16.dp)
            }
        }
    }
}

@Composable
private fun <T> Picker(
    label: String,
    shown: String,
    color: String?,
    options: List<Pair<String, T>>,
    manage: Pair<String, () -> Unit>? = null,
    onPick: (T) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    val c = Fennec.colors
    Box {
        BoxRow(label, onClick = { open = true }) {
            if (color != null) {
                Dot(projectColor(color))
                androidx.compose.foundation.layout.Spacer(Modifier.widthIn(min = 8.dp))
            }
            Text(shown, style = MaterialTheme.typography.bodyMedium, color = c.text)
        }
        DropdownMenu(open, onDismissRequest = { open = false }) {
            for ((name, value) in options) {
                DropdownMenuItem(text = { Text(name) }, onClick = {
                    open = false
                    onPick(value)
                })
            }
            manage?.let { (name, go) ->
                HorizontalDivider(color = c.divider)
                DropdownMenuItem(text = { Text(name, color = c.accentText) }, onClick = {
                    open = false
                    go()
                })
            }
        }
    }
}

@Composable
private fun RecordingNow(
    s: Recorder.Status.Recording,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onStop: () -> Unit,
) {
    val c = Fennec.colors
    Column(Modifier.fillMaxSize()) {
        Column {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 20.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Chip(if (s.paused) "Paused" else "Recording", c.accentSoft, c.accentText)
                Box(Modifier.weight(1f))
                Mono("%.1f MB saved".format(s.bytes / 1_048_576.0))
            }
            HorizontalDivider(color = c.divider)
        }
        Column(Modifier.weight(1f).padding(horizontal = 24.dp, vertical = 24.dp)) {
            Text(s.title, fontFamily = SourceSerif, fontSize = 24.sp, fontWeight = FontWeight.Medium, color = c.text)
            Gap(6.dp)
            Text(
                listOfNotNull(s.projectName ?: "Unsorted", s.templateName).joinToString(" · "),
                style = MaterialTheme.typography.bodySmall,
            )
            Column(
                Modifier.weight(1f).fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                Text(
                    clock(s.elapsedMs), fontFamily = PlexMono, fontSize = 64.sp, fontWeight = FontWeight.Medium,
                    color = c.text, modifier = Modifier.testTag("elapsed"),
                )
                Gap(22.dp)
                LevelBars(s.levels, active = !s.paused)
                Gap(22.dp)
                Text(
                    "Keeps recording with the screen off. Saved to this phone as you go.",
                    style = MaterialTheme.typography.bodySmall, textAlign = TextAlign.Center,
                    modifier = Modifier.widthIn(max = 260.dp),
                )
            }
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(28.dp, Alignment.CenterHorizontally),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (s.paused) {
                    RoundButton(R.drawable.ic_mic, "Resume", onResume)
                } else {
                    RoundButton(R.drawable.ic_pause, "Pause", onPause)
                }
                RecordButton(recording = true, onClick = onStop)
                Box(Modifier.widthIn(min = 64.dp))
            }
            Gap(8.dp)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(28.dp, Alignment.CenterHorizontally)) {
                Text(if (s.paused) "Resume" else "Pause", style = MaterialTheme.typography.bodySmall, modifier = Modifier.widthIn(min = 64.dp), textAlign = TextAlign.Center)
                Text("Stop", style = MaterialTheme.typography.labelLarge, modifier = Modifier.widthIn(min = 104.dp), textAlign = TextAlign.Center)
                Box(Modifier.widthIn(min = 64.dp))
            }
        }
    }
}

/** Day headings: TODAY, YESTERDAY, then dates. */
fun dayLabel(t: Long, now: Long = System.currentTimeMillis()): String {
    val day = { ms: Long -> Calendar.getInstance().apply { timeInMillis = ms }.let { it.get(Calendar.YEAR) * 1000 + it.get(Calendar.DAY_OF_YEAR) } }
    return when (day(now) - day(t)) {
        0 -> "Today"
        1 -> "Yesterday"
        else -> SimpleDateFormat("EEEE d MMMM", Locale.ENGLISH).format(Date(t))
    }
}

@Composable
fun RecordingsScreen(
    recordings: List<Recording>,
    projects: Map<Long, Project>,
    pairedName: String?,
    attention: Boolean,
    onOpen: (String) -> Unit,
    onSettings: () -> Unit,
) {
    val c = Fennec.colors
    Column(Modifier.fillMaxSize()) {
        Header("Recordings") { HeaderIconWithDot(R.drawable.ic_settings, "Settings", attention, onSettings) }
        if (recordings.isEmpty()) {
            Column(
                Modifier.weight(1f).fillMaxWidth().padding(32.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                Text("Your recordings appear here", style = MaterialTheme.typography.titleSmall)
                Gap(6.dp)
                Text(
                    "Record on the Record tab. Each recording goes to Fennec on your computer, which transcribes it.",
                    style = MaterialTheme.typography.bodySmall, textAlign = TextAlign.Center,
                )
            }
        } else {
            val groups = recordings.groupBy { dayLabel(it.recordedAt) }
            LazyColumn(Modifier.weight(1f).padding(horizontal = 20.dp)) {
                for ((day, rows) in groups) {
                    item(key = "day-$day") { SectionTitle(day, Modifier.padding(top = 12.dp)) }
                    items(rows, key = { it.id }) { r ->
                        RecordingRow(r, projects[r.projectId], pairedName != null) { onOpen(r.id) }
                    }
                }
            }
        }
    }
}

@Composable
private fun RecordingRow(r: Recording, project: Project?, paired: Boolean, onClick: () -> Unit) {
    val c = Fennec.colors
    Column(Modifier.fillMaxWidth().clickable(onClick = onClick).testTag("recording-${r.id}")) {
        Column(Modifier.padding(vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    r.title, fontFamily = SourceSerif, fontSize = 16.sp, fontWeight = FontWeight.Medium,
                    color = c.text, modifier = Modifier.weight(1f), maxLines = 1,
                )
                Mono(clock(r.durationMs))
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Dot(projectColor(project?.color), 7.dp)
                Text(
                    SimpleDateFormat("HH:mm", Locale.ROOT).format(Date(r.recordedAt)) + " · " +
                        (r.projectName ?: project?.name ?: "Unsorted"),
                    fontSize = 12.5.sp, color = c.muted, modifier = Modifier.weight(1f), maxLines = 1,
                )
                StateChip(r, paired)
            }
            if (r.state == SyncState.SENDING) ProgressLine(percent(r) / 100f)
        }
        HorizontalDivider(color = c.divider)
    }
}
