package io.github.fennec.recorder

import androidx.compose.ui.graphics.toArgb
import io.github.fennec.recorder.ui.theme.DarkTokens
import io.github.fennec.recorder.ui.theme.FennecColors
import io.github.fennec.recorder.ui.theme.LightTokens
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/** The theme is Fennec's: every colour token equals the desktop's CSS. */
class TokensTest {
    private fun css(name: String): Map<String, String> {
        val dir = System.getProperty("fennec.css.dir") ?: error("fennec.css.dir is not set")
        return Regex("""@define-color fx_(\w+) #([0-9A-Fa-f]{6});""")
            .findAll(File(dir, name).readText())
            .associate { it.groupValues[1] to it.groupValues[2].uppercase() }
    }

    private fun camel(snake: String) = snake.split('_').mapIndexed { i, p -> if (i == 0) p else p.replaceFirstChar(Char::uppercase) }.joinToString("")

    private fun check(file: String, tokens: FennecColors) {
        val desktop = css(file)
        assertTrue("no tokens read from $file", desktop.size >= 25)
        // Color is an inline class: each field holds its packed value.
        val ours = FennecColors::class.java.declaredFields.filter { it.type == Long::class.javaPrimitiveType }.associate { f ->
            f.isAccessible = true
            f.name to "%06X".format(androidx.compose.ui.graphics.Color(f.getLong(tokens).toULong()).toArgb() and 0xFFFFFF)
        }
        for ((name, hex) in desktop) {
            assertEquals("fx_$name in $file", hex, ours[camel(name)])
        }
        assertEquals("tokens the desktop does not have", desktop.keys.map(::camel).toSet(), ours.keys)
    }

    @Test
    fun `light colours are the desktop's`() = check("style-light.css", LightTokens)

    @Test
    fun `dark colours are the desktop's`() = check("style-dark.css", DarkTokens)
}
