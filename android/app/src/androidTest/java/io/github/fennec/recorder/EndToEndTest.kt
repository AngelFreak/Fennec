package io.github.fennec.recorder

import android.Manifest
import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import android.os.Build
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.rule.GrantPermissionRule
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/**
 * Against the real Fennec (android/e2e/run-e2e.sh starts it on the host,
 * receiving, and passes the link its QR code holds as the runner argument
 * `sendUri`): go through the welcome, record twice (once with a pause), open
 * the link as another camera app would, and see both recordings sent and
 * Fennec's projects arrive for the pickers. The host script checks what
 * arrived on the desktop side.
 */
@OptIn(ExperimentalTestApi::class)
@RunWith(AndroidJUnit4::class)
class EndToEndTest {
    @get:Rule val compose = createEmptyComposeRule()

    @get:Rule val permissions: GrantPermissionRule = if (Build.VERSION.SDK_INT >= 33) {
        GrantPermissionRule.grant(Manifest.permission.RECORD_AUDIO, Manifest.permission.POST_NOTIFICATIONS)
    } else {
        GrantPermissionRule.grant(Manifest.permission.RECORD_AUDIO)
    }

    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val shots by lazy {
        File(instrumentation.targetContext.getExternalFilesDir(null), "e2e").apply { mkdirs() }
    }

    private fun shot(name: String) {
        Thread.sleep(400)
        instrumentation.uiAutomation.takeScreenshot()?.let { bmp ->
            File(shots, "$name.png").outputStream().use { bmp.compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
    }

    private fun waitForText(text: String, seconds: Long = 30) =
        compose.waitUntilAtLeastOneExists(hasText(text, substring = true), seconds * 1000)

    private fun record(title: String, seconds: Long, pauseFor: Long = 0) {
        compose.onNodeWithTag("title").performTextInput(title)
        compose.onNodeWithContentDescription("Start recording").performClick()
        compose.waitUntilAtLeastOneExists(hasContentDescription("Stop and save"), 10_000)
        Thread.sleep(seconds * 1000 / 2)
        if (pauseFor > 0) {
            compose.onNodeWithContentDescription("Pause").performClick()
            waitForText("Paused")
            shot("paused")
            Thread.sleep(pauseFor * 1000)
            compose.onNodeWithContentDescription("Resume").performClick()
        } else {
            shot("recording")
        }
        Thread.sleep(seconds * 1000 / 2)
        compose.onNodeWithContentDescription("Stop and save").performClick()
        compose.waitUntilAtLeastOneExists(hasContentDescription("Start recording"), 10_000)
    }

    @Test
    fun recordAndSendByCode() {
        val encoded = InstrumentationRegistry.getArguments().getString("sendUri")
        assumeTrue("run through android/e2e/run-e2e.sh (no sendUri given)", encoded != null)
        // Base64 (URL-safe): the device's shell would split the link at "&".
        val link = String(android.util.Base64.decode(encoded, android.util.Base64.URL_SAFE or android.util.Base64.NO_PADDING))
        ActivityScenario.launch(MainActivity::class.java).use {
            // A first run: the welcome, ending with how sending works.
            waitForText("Get started")
            compose.onNodeWithText("Get started").performClick()
            waitForText("Microphone allowed")
            compose.onNodeWithText("Continue").performClick()
            waitForText("Receive from phone")
            shot("welcome-send")
            compose.onNodeWithText("Start recording").performClick()

            // Nothing read from Fennec yet: Unsorted and Fennec's default template.
            waitForText("NEW RECORDING")
            shot("record")
            record("E2E møde", seconds = 6)
            record("E2E med pause", seconds = 4, pauseFor = 3)

            compose.onNodeWithText("Recordings").performClick()
            compose.waitUntil(10_000) { compose.onAllNodesWithText("Not sent").fetchSemanticsNodes().size == 2 }
            waitForText("2 recordings not sent")
            shot("not-sent")

            // Fennec's code, opened from another camera app: sends at once.
            instrumentation.targetContext.startActivity(
                Intent(Intent.ACTION_VIEW, Uri.parse(link))
                    .setClassName(instrumentation.targetContext, MainActivity::class.java.name)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            )
            // Two short recordings may be sent before a screenshot can catch them going.
            compose.waitUntil(30_000) {
                listOf("Sending to", "Sent 2 recordings to").any { t ->
                    compose.onAllNodes(hasText(t, substring = true)).fetchSemanticsNodes().isNotEmpty()
                }
            }
            shot("sending")
            waitForText("Sent 2 recordings to", 120)
            shot("sent")
            compose.onNodeWithText("Done").performClick()

            compose.waitUntil(10_000) { compose.onAllNodesWithText("Sent").fetchSemanticsNodes().size == 2 }
            waitForText("Everything is sent")
            shot("recordings")

            compose.onNodeWithText("E2E møde").performClick()
            waitForText("Sent to")
            shot("detail")
            compose.onNodeWithContentDescription("Back to recordings").performClick()

            // The scan brought Fennec's projects for filing the next recording.
            compose.onNodeWithText("Record").performClick()
            waitForText("NEW RECORDING")
            compose.onNodeWithText("Unsorted").performClick()
            compose.waitUntilAtLeastOneExists(hasText("Kundemøder"), 10_000)
            shot("projects")
        }
    }
}
