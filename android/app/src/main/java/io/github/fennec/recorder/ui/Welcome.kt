package io.github.fennec.recorder.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.fennec.recorder.R
import io.github.fennec.recorder.ui.theme.Fennec
import io.github.fennec.recorder.ui.theme.SourceSerif

/** Steps of the first run: what the app does, the microphone, pairing (see [PairScreen]). */
const val WELCOME_STEPS = 3

/** The frame of a welcome step: step dots on top, the content, buttons below. */
@Composable
fun WelcomeFrame(
    step: Int,
    onSkip: (() -> Unit)?,
    buttons: @Composable () -> Unit,
    content: @Composable () -> Unit,
) {
    val c = Fennec.colors
    Column(Modifier.fillMaxSize().background(c.surface)) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 20.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            StepDots(WELCOME_STEPS, step)
            Box(Modifier.weight(1f))
            onSkip?.let { TextButton(it) { Text("Skip", color = c.muted) } }
        }
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 24.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) { content() }
        Column(Modifier.fillMaxWidth().padding(20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) { buttons() }
    }
}

@Composable
private fun Feature(icon: Int, title: String, text: String) {
    val c = Fennec.colors
    Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
        IconDisc(icon, 44.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(title, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.text)
            Text(text, fontSize = 13.5.sp, lineHeight = 20.sp, color = c.muted)
        }
    }
}

/** Step 1: what Fennec Recorder is for. */
@Composable
fun WelcomeIntro(onNext: () -> Unit) {
    val c = Fennec.colors
    WelcomeFrame(0, onSkip = null, buttons = { PrimaryButton("Get started", Modifier.fillMaxWidth(), onClick = onNext) }) {
        Gap(12.dp)
        FennecLogo(64.dp)
        Text(
            "Record here.\nTranscribe in Fennec.",
            fontFamily = SourceSerif, fontSize = 30.sp, lineHeight = 36.sp, fontWeight = FontWeight.Medium, color = c.text,
        )
        Text(
            "Fennec Recorder records meetings and interviews on your phone and hands them to Fennec on your computer.",
            style = MaterialTheme.typography.bodyLarge, color = c.textSoft,
        )
        Gap(4.dp)
        Feature(R.drawable.ic_mic, "Record anywhere", "With the screen off, for as long as you need. Pause when you need to.")
        Feature(R.drawable.ic_send, "Sent over your Wi-Fi", "Straight to Fennec, encrypted. Nothing passes through the internet.")
        Feature(R.drawable.ic_file, "Transcribed in Fennec", "Its Danish models turn each recording into a document, filed under your projects.")
    }
}

/** Step 2: the microphone (and notifications, asked with it). */
@Composable
fun WelcomeMicrophone(allowed: Boolean, denied: Boolean, onAllow: () -> Unit, onNext: () -> Unit) {
    val c = Fennec.colors
    WelcomeFrame(
        1, onSkip = onNext,
        buttons = {
            if (allowed) {
                PrimaryButton("Continue", Modifier.fillMaxWidth(), onClick = onNext)
            } else {
                PrimaryButton("Allow microphone", Modifier.fillMaxWidth(), onClick = onAllow)
            }
        },
    ) {
        Gap(12.dp)
        IconDisc(R.drawable.ic_mic, 64.dp)
        Text("The microphone", fontFamily = SourceSerif, fontSize = 28.sp, fontWeight = FontWeight.Medium, color = c.text)
        Text(
            "Fennec Recorder listens only while you record. A notification shows while it does, with Pause and Stop, so you can control it from the lock screen.",
            style = MaterialTheme.typography.bodyLarge, color = c.textSoft,
        )
        when {
            allowed -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Box(Modifier.size(22.dp).clip(RoundedCornerShape(11.dp)).background(c.ok), contentAlignment = Alignment.Center) {
                    FennecIcon(R.drawable.ic_check, Color.White, 12.dp)
                }
                Text("Microphone allowed", fontSize = 14.sp, fontWeight = FontWeight.Medium, color = c.text)
            }
            denied -> Text(
                "Not allowed. You can allow it later in Android's settings, or when you first press Record.",
                fontSize = 13.sp, color = c.error,
            )
        }
    }
}

/** Asks for the camera before scanning, in Fennec's style. */
@Composable
fun CameraPermissionCard(onAllow: () -> Unit) {
    val c = Fennec.colors
    Column(
        Modifier.fillMaxSize().clip(RoundedCornerShape(12.dp)).background(c.chrome)
            .border(1.dp, c.border, RoundedCornerShape(12.dp)).padding(24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
    ) {
        IconDisc(R.drawable.ic_camera, 56.dp)
        Text("Scan Fennec's code", fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.text)
        Text(
            "The camera is used only to read the pairing code.",
            fontSize = 13.sp, color = c.muted, textAlign = TextAlign.Center,
        )
        PrimaryButton("Allow camera", onClick = onAllow)
    }
}

/** The camera's picture with corner marks where the code goes. */
@Composable
fun Viewfinder(content: @Composable BoxScope.() -> Unit) {
    val accent = Fennec.colors.accent
    Box(
        Modifier.fillMaxSize().clip(RoundedCornerShape(12.dp)).background(Color(0xFF15171C))
            .drawWithContent {
                drawContent()
                val side = size.minDimension * 0.62f
                val left = (size.width - side) / 2
                val top = (size.height - side) / 2
                val arm = side * 0.18f
                val w = 3.dp.toPx()
                for ((x, y, dx, dy) in listOf(
                    listOf(left, top, 1f, 1f), listOf(left + side, top, -1f, 1f),
                    listOf(left, top + side, 1f, -1f), listOf(left + side, top + side, -1f, -1f),
                )) {
                    drawLine(accent, Offset(x, y), Offset(x + dx * arm, y), w, StrokeCap.Round)
                    drawLine(accent, Offset(x, y), Offset(x, y + dy * arm), w, StrokeCap.Round)
                }
            },
        contentAlignment = Alignment.Center,
        content = content,
    )
}
