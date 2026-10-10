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
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.RadioButtonDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.fennec.recorder.R
import io.github.fennec.recorder.data.DesktopInfo
import io.github.fennec.recorder.data.Project
import io.github.fennec.recorder.ui.theme.Fennec

/** A project being added (no id) or changed, and what saving said. */
data class ProjectDraft(
    val id: Long?,
    val name: String,
    val color: String,
    val template: String?,
    val saving: Boolean = false,
    val error: String? = null,
)

@Composable
fun ProjectsScreen(
    info: DesktopInfo,
    paired: Boolean,
    reachable: Boolean?,
    draft: ProjectDraft?,
    onBack: () -> Unit,
    onPair: () -> Unit,
    onEdit: (ProjectDraft?) -> Unit,
    onSave: (ProjectDraft) -> Unit,
) {
    val c = Fennec.colors
    Column(Modifier.fillMaxSize()) {
        Header("Projects", navigation = { HeaderIcon(R.drawable.ic_back, "Back", onBack) })
        if (!paired) {
            Empty(
                R.drawable.ic_file, "Pair with Fennec to see its projects",
                "Projects live in Fennec on your computer. Once paired you can add them here and file recordings under them.",
            ) { PrimaryButton("Pair with Fennec", onClick = onPair) }
            return@Column
        }
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                "Changes go to Fennec at once and show in its sidebar. To delete a project, use Fennec on your computer.",
                style = MaterialTheme.typography.bodySmall,
            )
            if (reachable == false) {
                Text("Fennec is not reachable right now, so projects cannot change.", fontSize = 13.sp, color = c.accentText)
            }
            Gap(4.dp)
            if (info.projects.isEmpty()) {
                Text("No projects yet.", fontSize = 14.sp, color = c.muted, modifier = Modifier.padding(vertical = 8.dp))
            } else {
                FennecBox {
                    info.projects.forEachIndexed { i, p ->
                        if (i > 0) HorizontalDivider(color = c.divider)
                        Row(
                            Modifier.fillMaxWidth().heightIn(min = 60.dp)
                                .clickable(role = Role.Button) {
                                    onEdit(ProjectDraft(p.id, p.name, p.color, p.defaultTemplate))
                                }
                                .padding(horizontal = 14.dp, vertical = 10.dp).testTag("project-${p.name}"),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            Dot(projectColor(p.color), 10.dp)
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text(p.name, fontSize = 14.sp, fontWeight = FontWeight.Medium, color = c.text)
                                Text(
                                    listOfNotNull(
                                        info.template(p.defaultTemplate)?.let { "${it.name} by default" },
                                        if (p.documents == 1) "1 document" else "${p.documents} documents",
                                    ).joinToString(" · "),
                                    fontSize = 12.5.sp, color = c.muted,
                                )
                            }
                            FennecIcon(R.drawable.ic_chevron, c.muted, 16.dp)
                        }
                    }
                }
            }
        }
        Box(Modifier.padding(20.dp)) {
            PrimaryButton("New project", Modifier.fillMaxWidth()) {
                val color = info.projectColors.getOrElse(info.projects.size % info.projectColors.size.coerceAtLeast(1)) { "#C2410C" }
                onEdit(ProjectDraft(null, "", color, null))
            }
        }
    }
    draft?.let { ProjectEditor(it, info, onChange = onEdit, onSave = onSave, onCancel = { onEdit(null) }) }
}

@Composable
private fun ProjectEditor(
    d: ProjectDraft,
    info: DesktopInfo,
    onChange: (ProjectDraft) -> Unit,
    onSave: (ProjectDraft) -> Unit,
    onCancel: () -> Unit,
) {
    val c = Fennec.colors
    AlertDialog(
        onDismissRequest = onCancel,
        containerColor = c.surface,
        title = { Text(if (d.id == null) "New project" else "Edit project") },
        text = { ProjectFields(d, info, onChange) },
        confirmButton = {
            if (d.saving) {
                CircularProgressIndicator(Modifier.size(20.dp), color = c.accent, strokeWidth = 2.dp)
            } else {
                TextButton({ onSave(d) }, enabled = d.name.isNotBlank()) {
                    Text(if (d.id == null) "Add" else "Save", color = if (d.name.isNotBlank()) c.accentText else c.faint)
                }
            }
        },
        dismissButton = { TextButton(onCancel) { Text("Cancel", color = c.muted) } },
    )
}

/** The editor's fields: name, colour, default template, and what saving said. */
@Composable
fun ProjectFields(d: ProjectDraft, info: DesktopInfo, onChange: (ProjectDraft) -> Unit) {
    val c = Fennec.colors
    var templates by remember { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
        OutlinedTextField(
            d.name, { onChange(d.copy(name = it, error = null)) }, label = { Text("Name") }, singleLine = true,
            colors = fieldColors(), modifier = Modifier.fillMaxWidth().testTag("project-name"),
        )
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Colour", style = MaterialTheme.typography.bodySmall)
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                for (col in info.projectColors) {
                    val on = col.equals(d.color, ignoreCase = true)
                    Box(
                        Modifier.size(36.dp).clip(CircleShape)
                            .border(if (on) 2.dp else 0.dp, if (on) c.text else c.surface, CircleShape)
                            .padding(4.dp).clip(CircleShape).background(projectColor(col))
                            .clickable(role = Role.RadioButton) { onChange(d.copy(color = col)) }
                            .semantics { contentDescription = "Colour $col"; selected = on },
                    )
                }
            }
        }
        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("Template for its recordings", style = MaterialTheme.typography.bodySmall)
            Box {
                FennecBox {
                    BoxRow("Template", first = true, onClick = { templates = true }) {
                        Text(info.template(d.template)?.name ?: "Fennec's default", fontSize = 14.sp, color = c.text)
                    }
                }
                DropdownMenu(templates, onDismissRequest = { templates = false }) {
                    DropdownMenuItem(text = { Text("Fennec's default") }, onClick = { templates = false; onChange(d.copy(template = null)) })
                    for (t in info.templates) {
                        DropdownMenuItem(text = { Text(t.name) }, onClick = { templates = false; onChange(d.copy(template = t.id)) })
                    }
                }
            }
        }
        d.error?.let { Text(it, color = c.error, fontSize = 13.sp, modifier = Modifier.testTag("project-error")) }
    }
}

@Composable
fun TemplatesScreen(
    info: DesktopInfo,
    paired: Boolean,
    phoneDefault: String?,
    onBack: () -> Unit,
    onPair: () -> Unit,
    onPick: (String?) -> Unit,
) {
    val c = Fennec.colors
    Column(Modifier.fillMaxSize()) {
        Header("Templates", navigation = { HeaderIcon(R.drawable.ic_back, "Back", onBack) })
        if (!paired) {
            Empty(
                R.drawable.ic_template, "Pair with Fennec to see its templates",
                "Templates are made in Fennec on your computer: the heading, the fields and how documents look.",
            ) { PrimaryButton("Pair with Fennec", onClick = onPair) }
            return@Column
        }
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                "New recordings use the template chosen here, unless their project has its own. Templates are edited in Fennec on your computer.",
                style = MaterialTheme.typography.bodySmall,
            )
            Gap(4.dp)
            SectionTitle("For new recordings")
            FennecBox {
                val fennecDefault = info.template(info.defaultTemplate)
                TemplateChoice(
                    "Fennec's default", fennecDefault?.let { "${it.name}, as Fennec sets it" } ?: "As Fennec sets it",
                    phoneDefault == null, first = true,
                ) { onPick(null) }
                for (t in info.templates) {
                    TemplateChoice(t.name, fieldSummary(t), phoneDefault == t.id) { onPick(t.id) }
                }
            }
        }
    }
}

@Composable
private fun TemplateChoice(title: String, note: String, selected: Boolean, first: Boolean = false, onPick: () -> Unit) {
    val c = Fennec.colors
    if (!first) HorizontalDivider(color = c.divider)
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp).clickable(role = Role.RadioButton, onClick = onPick)
            .padding(horizontal = 14.dp, vertical = 8.dp).testTag("template-$title"),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(selected, onPick, colors = RadioButtonDefaults.colors(selectedColor = c.accent, unselectedColor = c.faint))
        Column(Modifier.padding(start = 6.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, fontSize = 14.sp, fontWeight = FontWeight.Medium, color = c.text)
            Text(note, fontSize = 12.5.sp, color = c.muted)
        }
    }
}

@Composable
private fun Empty(icon: Int, title: String, text: String, action: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
    ) {
        IconDisc(icon, 56.dp)
        Text(title, style = MaterialTheme.typography.titleSmall, textAlign = TextAlign.Center)
        Text(text, style = MaterialTheme.typography.bodySmall, textAlign = TextAlign.Center)
        Gap(4.dp)
        action()
    }
}
