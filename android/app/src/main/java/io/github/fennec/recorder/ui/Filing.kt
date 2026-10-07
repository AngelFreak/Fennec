package io.github.fennec.recorder.ui

import io.github.fennec.recorder.data.DesktopInfo
import io.github.fennec.recorder.data.Project
import io.github.fennec.recorder.data.Template

/**
 * The template a new recording gets: the one picked for it, else its
 * project's default, else this phone's default. Null leaves it to Fennec,
 * which uses its own default (as for dictation).
 */
fun templateFor(picked: Template?, project: Project?, info: DesktopInfo, phoneDefault: String?): Template? =
    picked ?: info.template(project?.defaultTemplate) ?: info.template(phoneDefault)

/** What the Template row shows when nothing is picked: the template that will be used, and why. */
fun defaultTemplateLabel(project: Project?, info: DesktopInfo, phoneDefault: String?): String {
    info.template(project?.defaultTemplate)?.let { return "${it.name} · project default" }
    info.template(phoneDefault)?.let { return "${it.name} · default" }
    return info.template(info.defaultTemplate)?.let { "${it.name} · Fennec's default" } ?: "Fennec's default"
}

/** "Sagsnr., Dato, Udarbejdet af, Emne". */
fun fieldSummary(t: Template): String =
    if (t.fields.isEmpty()) "No header fields" else t.fields.joinToString(", ") { it.label }
