package io.github.fennec.recorder.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.fennec.recorder.R

/**
 * Fennec's colour tokens, the same values as the desktop's
 * `src/ui/style-light.css` and `style-dark.css` (a unit test compares them).
 * Field names are the CSS names without `fx_`, in camel case.
 */
@Immutable
data class FennecColors(
    val surface: Color,
    val chrome: Color,
    val inspector: Color,
    val chip: Color,
    val border: Color,
    val divider: Color,
    val dash: Color,
    val track: Color,
    val canvas: Color,
    val text: Color,
    val textSoft: Color,
    val muted: Color,
    val faint: Color,
    val preview: Color,
    val accent: Color,
    val accentHover: Color,
    val accentSoft: Color,
    val accentTint: Color,
    val accentText: Color,
    val unsureBorder: Color,
    val error: Color,
    val aiBg: Color,
    val aiText: Color,
    val aiDash: Color,
    val aiTint: Color,
    val netBg: Color,
    val netText: Color,
    val ok: Color,
    val ink: Color,
    val inkFg: Color,
)

val LightTokens = FennecColors(
    surface = Color(0xFFFFFFFF),
    chrome = Color(0xFFF6F7F9),
    inspector = Color(0xFFFAFAFB),
    chip = Color(0xFFF1F2F5),
    border = Color(0xFFDCE0E6),
    divider = Color(0xFFEEF0F3),
    dash = Color(0xFFB8BFCA),
    track = Color(0xFFE4E7EC),
    canvas = Color(0xFFE9ECF0),
    text = Color(0xFF15171C),
    textSoft = Color(0xFF2A2E37),
    muted = Color(0xFF5A6170),
    faint = Color(0xFF9AA1AE),
    preview = Color(0xFF6B7280),
    accent = Color(0xFFC2410C),
    accentHover = Color(0xFF9A3412),
    accentSoft = Color(0xFFFDEBDD),
    accentTint = Color(0xFFFFFBF8),
    accentText = Color(0xFF9A3412),
    unsureBorder = Color(0xFFF4C7A6),
    error = Color(0xFFB42318),
    aiBg = Color(0xFFE8EEFC),
    aiText = Color(0xFF1E3A8A),
    aiDash = Color(0xFF93A8E8),
    aiTint = Color(0xFFF4F7FE),
    netBg = Color(0xFFE6F4F1),
    netText = Color(0xFF0F5A53),
    ok = Color(0xFF0F766E),
    ink = Color(0xFF15171C),
    inkFg = Color(0xFFFFFFFF),
)

val DarkTokens = FennecColors(
    surface = Color(0xFF16181D),
    chrome = Color(0xFF1C1F25),
    inspector = Color(0xFF191B21),
    chip = Color(0xFF252930),
    border = Color(0xFF30353D),
    divider = Color(0xFF262A31),
    dash = Color(0xFF4A5059),
    track = Color(0xFF2C3038),
    canvas = Color(0xFF0F1114),
    text = Color(0xFFECEDF0),
    textSoft = Color(0xFFCFD3DA),
    muted = Color(0xFF9AA1AE),
    faint = Color(0xFF6B7280),
    preview = Color(0xFF8A919E),
    accent = Color(0xFFE0652C),
    accentHover = Color(0xFFF07A42),
    accentSoft = Color(0xFF3A2318),
    accentTint = Color(0xFF22191A),
    accentText = Color(0xFFF59A6B),
    unsureBorder = Color(0xFF6B3A22),
    error = Color(0xFFF97066),
    aiBg = Color(0xFF1E2A4A),
    aiText = Color(0xFFA9C0F5),
    aiDash = Color(0xFF4D64A8),
    aiTint = Color(0xFF18203A),
    netBg = Color(0xFF12302C),
    netText = Color(0xFF7FD3C6),
    ok = Color(0xFF34B3A2),
    ink = Color(0xFFECEDF0),
    inkFg = Color(0xFF16181D),
)

val LocalFennec = staticCompositionLocalOf { LightTokens }

@OptIn(ExperimentalTextApi::class)
private fun plexSans(weight: Int) = Font(
    R.font.ibm_plex_sans,
    weight = FontWeight(weight),
    variationSettings = FontVariation.Settings(FontVariation.weight(weight)),
)

@OptIn(ExperimentalTextApi::class)
private fun sourceSerif(weight: Int) = Font(
    R.font.source_serif_4,
    weight = FontWeight(weight),
    variationSettings = FontVariation.Settings(FontVariation.weight(weight)),
)

/** The interface face. */
val PlexSans = FontFamily(plexSans(400), plexSans(500), plexSans(600))

/** Timers, lengths and codes. */
val PlexMono = FontFamily(
    Font(R.font.ibm_plex_mono_regular, FontWeight.Normal),
    Font(R.font.ibm_plex_mono_medium, FontWeight.Medium),
)

/** Recording titles, as documents on the desktop. */
val SourceSerif = FontFamily(sourceSerif(400), sourceSerif(500), sourceSerif(600))

private fun typography(c: FennecColors): Typography {
    val base = TextStyle(fontFamily = PlexSans, color = c.text)
    return Typography(
        titleLarge = base.copy(fontSize = 22.sp, fontWeight = FontWeight.SemiBold),
        titleMedium = base.copy(fontSize = 17.sp, fontWeight = FontWeight.SemiBold),
        titleSmall = base.copy(fontSize = 14.sp, fontWeight = FontWeight.SemiBold),
        bodyLarge = base.copy(fontSize = 15.sp, lineHeight = 22.sp),
        bodyMedium = base.copy(fontSize = 14.sp, lineHeight = 21.sp),
        bodySmall = base.copy(fontSize = 13.sp, lineHeight = 19.sp, color = c.muted),
        labelLarge = base.copy(fontSize = 14.sp, fontWeight = FontWeight.Medium),
        labelMedium = base.copy(fontSize = 12.sp, fontWeight = FontWeight.Medium),
        labelSmall = base.copy(fontSize = 11.sp, fontWeight = FontWeight.Medium, letterSpacing = 0.9.sp),
    )
}

@Composable
fun FennecTheme(dark: Boolean = isSystemInDarkTheme(), content: @Composable () -> Unit) {
    val c = if (dark) DarkTokens else LightTokens
    val scheme = if (dark) {
        darkColorScheme(
            primary = c.accent, onPrimary = Color.White, primaryContainer = c.accentSoft,
            onPrimaryContainer = c.accentText, background = c.surface, onBackground = c.text,
            surface = c.surface, onSurface = c.text, surfaceVariant = c.chrome, onSurfaceVariant = c.muted,
            surfaceContainer = c.chrome, surfaceContainerHigh = c.chrome, surfaceContainerHighest = c.chip,
            outline = c.border, outlineVariant = c.divider, error = c.error, onError = Color.White,
            secondaryContainer = c.accentSoft, onSecondaryContainer = c.accentText,
        )
    } else {
        lightColorScheme(
            primary = c.accent, onPrimary = Color.White, primaryContainer = c.accentSoft,
            onPrimaryContainer = c.accentText, background = c.surface, onBackground = c.text,
            surface = c.surface, onSurface = c.text, surfaceVariant = c.chrome, onSurfaceVariant = c.muted,
            surfaceContainer = c.chrome, surfaceContainerHigh = c.chrome, surfaceContainerHighest = c.chip,
            outline = c.border, outlineVariant = c.divider, error = c.error, onError = Color.White,
            secondaryContainer = c.accentSoft, onSecondaryContainer = c.accentText,
        )
    }
    val shapes = Shapes(
        extraSmall = RoundedCornerShape(6.dp),
        small = RoundedCornerShape(8.dp),
        medium = RoundedCornerShape(10.dp),
        large = RoundedCornerShape(12.dp),
        extraLarge = RoundedCornerShape(28.dp),
    )
    CompositionLocalProvider(LocalFennec provides c) {
        MaterialTheme(colorScheme = scheme, typography = typography(c), shapes = shapes, content = content)
    }
}

object Fennec {
    val colors: FennecColors
        @Composable get() = LocalFennec.current
}
