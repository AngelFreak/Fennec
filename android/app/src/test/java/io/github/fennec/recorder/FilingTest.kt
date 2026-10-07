package io.github.fennec.recorder

import io.github.fennec.recorder.data.AppSettings
import io.github.fennec.recorder.data.DesktopInfo
import io.github.fennec.recorder.data.Project
import io.github.fennec.recorder.data.Template
import io.github.fennec.recorder.data.TemplateField
import io.github.fennec.recorder.ui.defaultTemplateLabel
import io.github.fennec.recorder.ui.fieldSummary
import io.github.fennec.recorder.ui.templateFor
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FilingTest {
    private val notat = Template("notat", "Notat", listOf(TemplateField("sagsnr", "Sagsnr."), TemplateField("dato", "Dato", "date")))
    private val referat = Template("moedereferat", "Mødereferat")
    private val interview = Template("interview", "Interview")
    private val info = DesktopInfo(
        projects = listOf(Project(1, "Kundemøder", defaultTemplate = "moedereferat"), Project(2, "Intern")),
        templates = listOf(notat, referat, interview),
        defaultTemplate = "notat",
    )

    @Test
    fun `a picked template wins, then the project's, then the phone's, else Fennec decides`() {
        val kunde = info.projects[0]
        val intern = info.projects[1]
        assertEquals(interview, templateFor(interview, kunde, info, "notat"))
        assertEquals(referat, templateFor(null, kunde, info, "interview"))
        assertEquals(interview, templateFor(null, intern, info, "interview"))
        assertNull(templateFor(null, intern, info, null))
        assertNull("a default Fennec no longer has", templateFor(null, null, info, "gone"))
    }

    @Test
    fun `the Template row says which template applies and why`() {
        assertEquals("Mødereferat · project default", defaultTemplateLabel(info.projects[0], info, "interview"))
        assertEquals("Interview · default", defaultTemplateLabel(info.projects[1], info, "interview"))
        assertEquals("Notat · Fennec's default", defaultTemplateLabel(null, info, null))
        assertEquals("Fennec's default", defaultTemplateLabel(null, DesktopInfo(), null))
    }

    @Test
    fun `fields read as a list`() {
        assertEquals("Sagsnr., Dato", fieldSummary(notat))
        assertEquals("No header fields", fieldSummary(referat))
    }

    @Test
    fun `new installs keep audio in high quality and keep recordings`() {
        val d = AppSettings()
        assertTrue(d.highQuality)
        assertEquals(0, d.keepDays)
        assertTrue(d.unmeteredOnly)
    }
}
