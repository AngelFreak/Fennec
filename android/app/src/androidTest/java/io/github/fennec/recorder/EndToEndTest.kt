package io.github.fennec.recorder

import android.Manifest
import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import android.os.Build
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.onLast
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
 * Against the real Fennec (android/e2e/run-e2e.sh starts it on the host and
 * passes its pairing link as the runner argument `pairUri`): pair, record,
 * pause, send, see both recordings transcribed, then unpair. The host script
 * checks what arrived on the desktop side.
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
    fun pairRecordSendAndUnpair() {
        val encoded = InstrumentationRegistry.getArguments().getString("pairUri")
        assumeTrue("run through android/e2e/run-e2e.sh (no pairUri given)", encoded != null)
        // Base64 (URL-safe): the device's shell would split the link at "&".
        val link = String(android.util.Base64.decode(encoded, android.util.Base64.URL_SAFE or android.util.Base64.NO_PADDING))
        val open = Intent(Intent.ACTION_VIEW, Uri.parse(link))
            .setClassName(instrumentation.targetContext, MainActivity::class.java.name)
        ActivityScenario.launch<MainActivity>(open).use {
            // The link asks before connecting.
            waitForText("Pair with")
            shot("pair-confirm")
            compose.onNodeWithText("Pair").performClick()
            waitForText("Paired with", 60)
            shot("paired")
            compose.onNodeWithText("Start recording").performClick()

            // This phone's default template for recordings without a project default.
            waitForText("NEW RECORDING")
            compose.onNodeWithContentDescription("Settings").performClick()
            compose.onNodeWithText("Templates").performClick()
            waitForText("Afhøringsrapport", 20)
            compose.onNodeWithTag("template-Afhøringsrapport").performClick()
            shot("templates")
            compose.onNodeWithContentDescription("Back").performClick()

            // A project made on the phone, with its own default template.
            compose.onNodeWithText("Projects").performClick()
            compose.onNodeWithText("New project").performClick()
            compose.onNodeWithTag("project-name").performTextInput("Fra telefonen")
            compose.onNodeWithText("Fennec's default").performClick()
            compose.onAllNodesWithText("Mødereferat").onLast().performClick()
            shot("project-editor")
            compose.onNodeWithText("Add").performClick()
            compose.waitUntilAtLeastOneExists(hasTestTag("project-Fra telefonen"), 20_000)
            shot("projects")
            compose.onNodeWithContentDescription("Back").performClick()
            compose.onNodeWithContentDescription("Back").performClick()

            // Into the new project: its default template applies.
            waitForText("NEW RECORDING")
            compose.onNodeWithText("Unsorted").performClick()
            compose.onAllNodesWithText("Fra telefonen").onLast().performClick()
            waitForText("Mødereferat · project default")
            shot("record")
            record("E2E møde", seconds = 6)

            // Unsorted: this phone's default applies.
            compose.onNodeWithText("Fra telefonen").performClick()
            compose.onAllNodesWithText("Unsorted").onLast().performClick()
            waitForText("Afhøringsrapport · default")
            record("E2E med pause", seconds = 4, pauseFor = 3)

            compose.onNodeWithText("Recordings").performClick()
            compose.waitUntil(120_000) {
                compose.onAllNodesWithText("Transcribed").fetchSemanticsNodes().size == 2
            }
            shot("transcribed")

            compose.onNodeWithText("E2E møde").performClick()
            waitForText("Sent to")
            shot("detail")
            compose.onNodeWithContentDescription("Back to recordings").performClick()

            compose.onNodeWithContentDescription("Settings").performClick()
            compose.onNodeWithText("Unpair").performClick()
            compose.onAllNodesWithText("Unpair").onLast().performClick()
            waitForText("Not paired")
            shot("unpaired")
        }
    }
}
