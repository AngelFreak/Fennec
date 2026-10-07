package io.github.fennec.recorder.ui

import androidx.annotation.DrawableRes
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.fennec.recorder.R
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.ui.theme.Fennec
import io.github.fennec.recorder.ui.theme.PlexMono
import java.util.Locale

/**
 * Fennec's logo as the desktop draws it (`logo()` in src/ui/window.rs): an
 * accent square with rounded corners and a white mark, fox ears that read
 * as an M.
 */
@Composable
fun FennecLogo(size: Dp = 28.dp) {
    val accent = Fennec.colors.accent
    Canvas(Modifier.size(size).semantics { contentDescription = "Fennec" }) {
        val u = this.size.width / 26f
        drawRoundRect(accent, cornerRadius = CornerRadius(7 * u, 7 * u))
        // M4 20 L7 4 L12 12 L17 4 L20 20 Z in a 24-unit box drawn 16 units wide.
        val s = 16f / 24f * u
        val o = 5 * u
        val mark = Path().apply {
            moveTo(o + 4 * s, o + 20 * s)
            lineTo(o + 7 * s, o + 4 * s)
            lineTo(o + 12 * s, o + 12 * s)
            lineTo(o + 17 * s, o + 4 * s)
            lineTo(o + 20 * s, o + 20 * s)
            close()
        }
        drawPath(mark, Color.White)
    }
}

@Composable
fun FennecIcon(@DrawableRes id: Int, tint: Color, size: Dp = 20.dp, description: String? = null) {
    Icon(painterResource(id), contentDescription = description, tint = tint, modifier = Modifier.size(size))
}

/** The bar at the top of each screen. */
@Composable
fun Header(
    title: String,
    navigation: (@Composable () -> Unit)? = null,
    actions: @Composable RowScope.() -> Unit = {},
) {
    val c = Fennec.colors
    Column {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 64.dp)
                .padding(start = if (navigation == null) 20.dp else 8.dp, end = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            navigation?.invoke()
            Text(title, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
            actions()
        }
        HorizontalDivider(color = c.divider)
    }
}

@Composable
fun HeaderIcon(@DrawableRes id: Int, description: String, onClick: () -> Unit) {
    IconButton(onClick = onClick) { FennecIcon(id, Fennec.colors.muted, description = description) }
}

/** Spaced capitals above a group, as the desktop sidebar. */
@Composable
fun SectionTitle(text: String, modifier: Modifier = Modifier) {
    Text(
        text.uppercase(Locale.ROOT),
        style = MaterialTheme.typography.labelSmall,
        color = Fennec.colors.muted,
        modifier = modifier.padding(top = 6.dp, bottom = 6.dp),
    )
}

/** A bordered box of rows. */
@Composable
fun FennecBox(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    Column(
        modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp))
            .border(1.dp, Fennec.colors.border, RoundedCornerShape(10.dp)),
        content = content,
    )
}

/** A row in a [FennecBox]: label on the left, value on the right. */
@Composable
fun BoxRow(
    label: String,
    first: Boolean = false,
    onClick: (() -> Unit)? = null,
    value: @Composable RowScope.() -> Unit,
) {
    val c = Fennec.colors
    if (!first) HorizontalDivider(color = c.divider)
    Row(
        Modifier.fillMaxWidth().heightIn(min = 52.dp)
            .let { if (onClick != null) it.clickable(role = Role.Button, onClick = onClick) else it }
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, style = MaterialTheme.typography.bodySmall, modifier = Modifier.width(76.dp))
        Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically, content = value)
        if (onClick != null) FennecIcon(R.drawable.ic_chevron, c.muted, 16.dp)
    }
}

@Composable
fun Dot(color: Color, size: Dp = 8.dp) {
    Box(Modifier.size(size).clip(CircleShape).background(color))
}

/** Parses `#RRGGBB` project colours. */
fun projectColor(hex: String?): Color =
    runCatching { Color(android.graphics.Color.parseColor(hex)) }.getOrDefault(Color(0xFF9AA1AE))

@Composable
fun Chip(text: String, background: Color, foreground: Color, icon: Int? = null) {
    Row(
        Modifier.clip(RoundedCornerShape(999.dp)).background(background).padding(horizontal = 9.dp, vertical = 3.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        icon?.let { FennecIcon(it, foreground, 12.dp) }
        Text(text, fontSize = 11.5.sp, fontWeight = FontWeight.Medium, color = foreground)
    }
}

/** Where a recording is, as a chip. */
@Composable
fun StateChip(r: Recording, paired: Boolean) {
    val c = Fennec.colors
    when (r.state) {
        SyncState.RECORDING -> Chip("Recording", c.accentSoft, c.accentText)
        SyncState.WAITING -> Chip(if (paired) "Waiting for Fennec" else "Not paired", c.chip, c.muted)
        SyncState.SENDING -> Chip("Sending ${percent(r)}%", c.accentSoft, c.accentText)
        SyncState.QUEUED -> Chip("Queued", c.aiBg, c.aiText)
        SyncState.TRANSCRIBING -> Chip("Transcribing", c.aiBg, c.aiText)
        SyncState.DONE -> Chip("Transcribed", c.netBg, c.netText, R.drawable.ic_check)
        SyncState.FAILED -> Chip("Failed", c.error.copy(alpha = 0.14f), c.error)
    }
}

fun percent(r: Recording): Int = if (r.size > 0) (r.sentBytes * 100 / r.size).toInt().coerceIn(0, 100) else 0

@Composable
fun ProgressLine(fraction: Float, modifier: Modifier = Modifier, color: Color = Fennec.colors.accent) {
    Box(modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(2.dp)).background(Fennec.colors.track)) {
        Box(
            Modifier.fillMaxWidth(fraction.coerceIn(0f, 1f)).height(4.dp).clip(RoundedCornerShape(2.dp))
                .background(color),
        )
    }
}

/** `12:48`, or `1:02:03` past an hour. */
fun clock(ms: Long): String {
    val s = ms / 1000
    return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, s / 60 % 60, s % 60) else "%02d:%02d".format(s / 60, s % 60)
}

@Composable
fun Mono(text: String, size: Int = 12, color: Color = Fennec.colors.muted, weight: FontWeight = FontWeight.Normal) {
    Text(text, fontFamily = PlexMono, fontSize = size.sp, color = color, fontWeight = weight, maxLines = 1)
}

/** Loudness as bars; the newest on the right. */
@Composable
fun LevelBars(levels: List<Float>, active: Boolean, count: Int = 28, modifier: Modifier = Modifier) {
    val c = Fennec.colors
    val shown = (List((count - levels.size).coerceAtLeast(0)) { 0f } + levels).takeLast(count)
    Row(
        modifier.height(56.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        for (l in shown) {
            val h = (6 + 50 * kotlin.math.sqrt(l)).dp
            Box(
                Modifier.width(4.dp).height(h).clip(RoundedCornerShape(2.dp))
                    .background(if (active && l > 0.01f) c.accent else c.track),
            )
        }
    }
}

/**
 * The big round button: one solid circle, a microphone to start and a
 * square to stop. While recording a soft halo breathes around it.
 */
@Composable
fun RecordButton(recording: Boolean, onClick: () -> Unit) {
    val c = Fennec.colors
    val halo by rememberInfiniteTransition(label = "halo").animateFloat(
        initialValue = 0.86f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(1100), RepeatMode.Reverse),
        label = "halo",
    )
    Box(Modifier.size(104.dp), contentAlignment = Alignment.Center) {
        if (recording) {
            Box(Modifier.size(104.dp).scale(halo).clip(CircleShape).background(c.accent.copy(alpha = 0.16f)))
        }
        Box(
            Modifier.size(84.dp).clip(CircleShape).background(c.accent)
                .clickable(role = Role.Button, onClick = onClick)
                .semantics { contentDescription = if (recording) "Stop and save" else "Start recording" },
            contentAlignment = Alignment.Center,
        ) {
            if (recording) {
                Box(Modifier.size(26.dp).clip(RoundedCornerShape(6.dp)).background(Color.White))
            } else {
                FennecIcon(R.drawable.ic_mic, Color.White, 32.dp)
            }
        }
    }
}

/** A secondary round control beside the record button: filled, no outline. */
@Composable
fun RoundButton(@DrawableRes icon: Int, description: String, onClick: () -> Unit) {
    val c = Fennec.colors
    Box(
        Modifier.size(64.dp).clip(CircleShape).background(c.chip)
            .clickable(role = Role.Button, onClick = onClick).semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) { FennecIcon(icon, c.text, 24.dp) }
}

/** An icon on a soft accent disc, for headings of welcome steps and empty states. */
@Composable
fun IconDisc(@DrawableRes icon: Int, size: Dp = 56.dp) {
    val c = Fennec.colors
    Box(Modifier.size(size).clip(CircleShape).background(c.accentSoft), contentAlignment = Alignment.Center) {
        FennecIcon(icon, c.accentText, size * 0.45f)
    }
}

/** Where in a series of steps one is: the current step is a longer pill. */
@Composable
fun StepDots(count: Int, current: Int) {
    val c = Fennec.colors
    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
        repeat(count) { i ->
            Box(
                Modifier.height(6.dp).width(if (i == current) 22.dp else 6.dp).clip(RoundedCornerShape(3.dp))
                    .background(if (i == current) c.accent else c.dash),
            )
        }
    }
}

@Composable
fun PrimaryButton(text: String, modifier: Modifier = Modifier, enabled: Boolean = true, onClick: () -> Unit) {
    Button(
        onClick, modifier.heightIn(min = 44.dp), enabled = enabled, shape = RoundedCornerShape(8.dp),
        colors = ButtonDefaults.buttonColors(containerColor = Fennec.colors.accent, contentColor = Color.White),
    ) { Text(text, style = MaterialTheme.typography.labelLarge, color = Color.White, fontWeight = FontWeight.SemiBold) }
}

@Composable
fun SecondaryButton(
    text: String,
    modifier: Modifier = Modifier,
    color: Color = Fennec.colors.text,
    onClick: () -> Unit,
) {
    OutlinedButton(
        onClick, modifier.heightIn(min = 44.dp), shape = RoundedCornerShape(8.dp),
        border = BorderStroke(1.dp, Fennec.colors.border),
    ) { Text(text, style = MaterialTheme.typography.labelLarge, color = color) }
}

/** The paired computer and whether it answered lately. */
@Composable
fun ContactLine(name: String?, reachable: Boolean?, trailing: String? = null, onPair: () -> Unit = {}) {
    val c = Fennec.colors
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.chrome)
            .let { if (name == null) it.clickable(onClick = onPair) else it }
            .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Dot(
            when {
                name == null -> c.faint
                reachable == false -> c.accent
                else -> c.ok
            },
        )
        Text(
            when {
                name == null -> "Not paired. Pair with Fennec to send recordings."
                reachable == false -> "$name is not reachable right now"
                reachable == true -> "Paired with $name"
                else -> "Paired with $name"
            },
            fontSize = 12.5.sp, color = c.muted, modifier = Modifier.weight(1f), maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
        trailing?.let { Mono(it) }
    }
}

@Composable
fun Gap(h: Dp) = Spacer(Modifier.height(h))
