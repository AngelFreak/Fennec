package io.github.fennec.recorder

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onRoot
import com.github.takahirom.roborazzi.captureRoboImage
import io.github.fennec.recorder.data.AppSettings
import io.github.fennec.recorder.data.DesktopInfo
import io.github.fennec.recorder.data.Paired
import io.github.fennec.recorder.data.Project
import io.github.fennec.recorder.data.Recording
import io.github.fennec.recorder.data.SyncState
import io.github.fennec.recorder.data.Template
import io.github.fennec.recorder.net.PairingTarget
import io.github.fennec.recorder.record.Recorder
import io.github.fennec.recorder.ui.DetailScreen
import io.github.fennec.recorder.ui.NextRecording
import io.github.fennec.recorder.ui.PairScreen
import io.github.fennec.recorder.ui.PairUi
import io.github.fennec.recorder.ui.Playback
import io.github.fennec.recorder.ui.RecordScreen
import io.github.fennec.recorder.ui.RecordingsScreen
import io.github.fennec.recorder.ui.SettingsScreen
import io.github.fennec.recorder.ui.theme.Fennec
import io.github.fennec.recorder.ui.theme.FennecTheme
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.ParameterizedRobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.util.Calendar

/**
 * Every screen, light and dark, compared with the baselines in
 * src/test/screenshots (record new ones with `./gradlew recordRoborazziDebug`).
 */
@RunWith(ParameterizedRobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "w392dp-h850dp-xxhdpi")
class ScreenshotTest(private val dark: Boolean) {
    companion object {
        @JvmStatic
        @ParameterizedRobolectricTestRunner.Parameters(name = "dark={0}")
        fun modes() = listOf(arrayOf<Any>(false), arrayOf<Any>(true))
    }

    @get:Rule val compose = createComposeRule()

    init {
        // Dates on screen depend on the time zone; screenshots must not.
        java.util.TimeZone.setDefault(java.util.TimeZone.getTimeZone("Europe/Copenhagen"))
    }

    private val info = DesktopInfo(
        listOf(Project(3, "Kundemøder", "#2F6F4E"), Project(4, "Intern", "#4A6FA5")),
        listOf(Template("notat", "Notat"), Template("interview", "Interview")),
    )

    /** Today at the given time, so day headings and times stay the same. */
    private fun today(h: Int, m: Int, daysAgo: Int = 0) = Calendar.getInstance().apply {
        add(Calendar.DAY_OF_YEAR, -daysAgo)
        set(Calendar.HOUR_OF_DAY, h); set(Calendar.MINUTE, m); set(Calendar.SECOND, 0); set(Calendar.MILLISECOND, 0)
    }.timeInMillis

    private fun rec(id: String, title: String, at: Long, ms: Long, state: SyncState, project: Project?, sent: Double = 0.0) =
        Recording(
            id = id, title = title, recordedAt = at, durationMs = ms, projectId = project?.id, projectName = project?.name,
            fileName = "$id.aac", size = ms * 6, sha256 = "x", state = state, sentBytes = (ms * 6 * sent).toLong(),
            deliveredAt = if (state.delivered) at else null,
        )

    private val list by lazy {
        val k = info.projects[0]
        listOf(
            rec("a", "Teammøde", today(15, 10), 1_915_000, SyncState.WAITING, info.projects[1]),
            rec("b", "Interview, Aarhus", today(14, 2), 1_083_000, SyncState.SENDING, k, 0.64),
            rec("c", "Møde med Jensen", today(10, 30), 2_530_000, SyncState.TRANSCRIBING, k),
            rec("d", "Kundebesøg, Vejle", today(13, 15, 1), 3_321_000, SyncState.DONE, k),
            rec("e", "Borgermøde", today(19, 0, 1), 4_360_000, SyncState.FAILED, null).copy(error = "Fennec could not read the audio."),
        )
    }

    private fun shot(name: String, content: @Composable () -> Unit) {
        compose.setContent {
            FennecTheme(dark = dark) {
                Box(Modifier.fillMaxSize().background(Fennec.colors.surface)) { content() }
            }
        }
        compose.onRoot().captureRoboImage("src/test/screenshots/$name-${if (dark) "dark" else "light"}.png")
    }

    @Test
    fun record() = shot("record") {
        RecordScreen(
            Recorder.Status.Idle, NextRecording("Møde med Jensen", info.projects[0], info.templates[0]), info,
            "fennec-desktop", true, {}, {}, {}, {}, {}, {}, {},
        )
    }

    @Test
    fun recording() = shot("recording") {
        val levels = listOf(.05f, .2f, .5f, .3f, .7f, .9f, .4f, .15f, .6f, .95f, .5f, .3f, .1f, .35f, .8f, .55f, .25f, .08f, .4f, .65f, .2f, .05f)
        RecordScreen(
            Recorder.Status.Recording("r", "Møde med Jensen", "Kundemøder", "Mødereferat", 768_000, false, 4_600_000, levels),
            NextRecording(), info, "fennec-desktop", true, {}, {}, {}, {}, {}, {}, {},
        )
    }

    @Test
    fun recordings() = shot("recordings") {
        RecordingsScreen(list, info.projects.associateBy { it.id }, "fennec-desktop", true, today(14, 47), {}, {}, {})
    }

    @Test
    fun detail() = shot("detail") {
        DetailScreen(
            list[1].copy(recordedAt = 1_791_295_320_000), info, "fennec-desktop", Playback(false, 238_000),
            {}, {}, {}, {}, {}, {}, {},
        )
    }

    @Test
    fun pairWaiting() = shot("pair-waiting") {
        PairScreen(
            PairUi.Waiting(PairingTarget("192.168.1.20:47130", "73051148", "pin", "fennec-desktop"), "4821 9306"),
            camera = {}, {}, { _, _ -> }, {}, {}, {},
        )
    }

    @Test
    fun pairScan() = shot("pair-scan") {
        PairScreen(
            PairUi.Scan,
            camera = { Text("camera", color = Color.White) }, {}, { _, _ -> }, {}, {}, {},
        )
    }

    @Test
    fun settings() = shot("settings") {
        SettingsScreen(
            Paired("fennec-desktop", "abc", "192.168.1.20:47130", "pin", 7, "s"), AppSettings(), "0.1.0",
            {}, {}, {}, {},
        )
    }
}
