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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.fennec.recorder.R
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.sync.SendUi
import io.github.fennec.recorder.ui.theme.Fennec
import io.github.fennec.recorder.ui.theme.SourceSerif

/** What to do in Fennec before scanning. */
const val HOW_TO_SEND =
    "To send recordings, open Fennec on your computer, choose Receive from phone, and scan the code with Send to Fennec."

private fun recordingsText(n: Int) = if (n == 1) "1 recording" else "$n recordings"

/**
 * Sending to Fennec: the camera until a code is read, then each recording
 * as it goes, then what got there. Leaving stops sending.
 */
@Composable
fun SendScreen(
    ui: SendUi,
    recordings: List<Recording>,
    camera: @Composable () -> Unit,
    onScanAgain: () -> Unit,
    onClose: () -> Unit,
) {
    val c = Fennec.colors
    // The phone must stay awake while recordings go.
    if (ui is SendUi.Connecting || ui is SendUi.Sending) {
        val view = LocalView.current
        DisposableEffect(view) {
            view.keepScreenOn = true
            onDispose { view.keepScreenOn = false }
        }
    }
    Column(Modifier.fillMaxSize()) {
        Header("Send to Fennec", navigation = { HeaderIcon(R.drawable.ic_close, "Close", onClose) })
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            when (ui) {
                SendUi.Scan -> {
                    val unsent = recordings.unsentCount()
                    Text(
                        "On your computer, open Fennec and choose Receive from phone. Point the camera at the code it shows.",
                        style = MaterialTheme.typography.bodyMedium, color = c.textSoft,
                    )
                    Box(Modifier.fillMaxWidth().height(300.dp), contentAlignment = Alignment.Center) { camera() }
                    Text(
                        if (unsent > 0) {
                            "${recordingsText(unsent)} not sent yet. All of them go."
                        } else {
                            "Everything is sent. Scanning still loads Fennec's projects and templates."
                        },
                        fontSize = 13.5.sp, color = c.text, modifier = Modifier.testTag("send-count"),
                    )
                    SameNetworkNote()
                }
                is SendUi.Connecting -> Card {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        CircularProgressIndicator(Modifier.size(16.dp), color = c.accent, strokeWidth = 2.dp, trackColor = c.track)
                        Text("Connecting to ${ui.name ?: "Fennec"}", fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.text)
                    }
                    Mono(ui.address, 13, c.textSoft)
                }
                is SendUi.Sending -> {
                    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Text("Sending to ${ui.name}", fontFamily = SourceSerif, fontSize = 24.sp, fontWeight = FontWeight.Medium, color = c.text)
                        Text(
                            if (ui.ids.isEmpty()) "Nothing to send" else "${ui.sent} of ${ui.ids.size} sent · keep this screen open",
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    if (ui.ids.isNotEmpty()) {
                        val byId = recordings.associateBy { it.id }
                        FennecBox {
                            ui.ids.forEachIndexed { i, id ->
                                if (i > 0) HorizontalDivider(color = c.divider)
                                byId[id]?.let { SendRow(it, current = id == ui.current) }
                            }
                        }
                    }
                }
                is SendUi.Done -> {
                    Card {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Box(Modifier.size(22.dp).clip(CircleShape).background(c.ok), contentAlignment = Alignment.Center) {
                                FennecIcon(R.drawable.ic_check, Color.White, 12.dp)
                            }
                            Text(
                                if (ui.sent == 0 && ui.failed == 0) "Up to date with ${ui.name}" else "Sent ${recordingsText(ui.sent)} to ${ui.name}",
                                fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.text, modifier = Modifier.testTag("send-done"),
                            )
                        }
                        Text(
                            when {
                                ui.failed > 0 ->
                                    "${recordingsText(ui.failed)} could not be sent. ${if (ui.failed == 1) "It is" else "They are"} sent again with the next code you scan."
                                ui.sent == 0 -> "Nothing was waiting. Fennec's projects and templates are loaded for filing new recordings."
                                else -> "Fennec transcribes them in its Files queue. You can close Receive from phone on the computer."
                            },
                            fontSize = 13.sp, color = if (ui.failed > 0) c.error else c.muted,
                        )
                    }
                }
                is SendUi.Failed -> Card {
                    Text(if (ui.sent > 0) "Sent ${recordingsText(ui.sent)}, then stopped" else "Not sent", fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.text)
                    Text(ui.message, fontSize = 13.sp, color = c.error, modifier = Modifier.testTag("send-error"))
                }
            }
        }
        Box(Modifier.padding(20.dp)) {
            when (ui) {
                is SendUi.Done -> PrimaryButton("Done", Modifier.fillMaxWidth(), onClick = onClose)
                is SendUi.Failed -> PrimaryButton("Scan again", Modifier.fillMaxWidth(), onClick = onScanAgain)
                is SendUi.Connecting, is SendUi.Sending -> SecondaryButton("Stop", Modifier.fillMaxWidth(), onClick = onClose)
                SendUi.Scan -> {}
            }
        }
    }
}

/** One recording in a transfer; the one going now stands out. */
@Composable
private fun SendRow(r: Recording, current: Boolean) {
    val c = Fennec.colors
    Column(
        Modifier.fillMaxWidth().background(if (current) c.accentSoft else Color.Transparent)
            .padding(horizontal = 14.dp, vertical = 12.dp).testTag("send-${r.id}"),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                r.title, fontFamily = SourceSerif, fontSize = 15.sp, fontWeight = FontWeight.Medium, color = c.text,
                maxLines = 1, modifier = Modifier.weight(1f),
            )
            when {
                r.state.delivered -> Chip("Sent", c.netBg, c.netText, R.drawable.ic_check)
                current -> Mono("${percent(r)}%", 12, c.accentText)
                r.state == SyncState.FAILED -> Chip("Failed", c.error.copy(alpha = 0.14f), c.error)
                else -> Text("Waiting", fontSize = 12.5.sp, color = c.muted)
            }
        }
        if (current && !r.state.delivered) {
            Mono("%.1f of %.1f MB".format(r.sentBytes / 1_048_576.0, r.size / 1_048_576.0), 11)
            ProgressLine(percent(r) / 100f)
        }
        if (r.state == SyncState.FAILED) r.error?.let { Text(it, fontSize = 12.5.sp, color = c.error) }
    }
}

/** A bordered card for the Send screen's messages. */
@Composable
private fun Card(content: @Composable () -> Unit) {
    val c = Fennec.colors
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(c.chrome)
            .border(1.dp, c.border, RoundedCornerShape(10.dp)).padding(horizontal = 16.dp, vertical = 14.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) { content() }
}

/** Top of the recordings list: how many wait, and the way to send them. */
@Composable
fun SendBar(unsent: Int, onSend: () -> Unit) {
    val c = Fennec.colors
    if (unsent == 0) {
        Row(
            Modifier.fillMaxWidth().padding(vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            FennecIcon(R.drawable.ic_check, c.netText, 16.dp)
            Text("Everything is sent", fontSize = 13.5.sp, color = c.muted, modifier = Modifier.weight(1f))
            Text(
                "Send to Fennec", fontSize = 13.5.sp, color = c.accentText, fontWeight = FontWeight.Medium,
                modifier = Modifier.clip(RoundedCornerShape(6.dp)).clickable(role = Role.Button, onClick = onSend)
                    .padding(horizontal = 6.dp, vertical = 4.dp),
            )
        }
        return
    }
    Row(
        Modifier.fillMaxWidth().padding(vertical = 12.dp).clip(RoundedCornerShape(10.dp)).background(c.accentSoft)
            .padding(start = 14.dp, end = 10.dp, top = 10.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text("${recordingsText(unsent)} not sent", fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = c.accentText)
            Text("Scan the code in Fennec to send them.", fontSize = 12.5.sp, color = c.text)
        }
        PrimaryButton("Send to Fennec", onClick = onSend)
    }
}

/** The Record screen's way to send, with how many wait; nothing when all are sent. */
@Composable
fun SendAction(unsent: Int, onSend: () -> Unit) {
    if (unsent == 0) return
    val c = Fennec.colors
    Row(
        Modifier.heightIn(min = 36.dp).clip(RoundedCornerShape(999.dp)).background(c.accentSoft)
            .clickable(role = Role.Button, onClick = onSend)
            .semantics(mergeDescendants = true) { contentDescription = "Send to Fennec, ${recordingsText(unsent)} not sent" }
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        FennecIcon(R.drawable.ic_send, c.accentText, 16.dp)
        Text("Send $unsent", fontSize = 13.sp, fontWeight = FontWeight.SemiBold, color = c.accentText)
    }
}
