package io.github.fennec.recorder.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.RadioButtonDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.fennec.recorder.R
import io.github.fennec.recorder.data.DesktopInfo
import io.github.fennec.recorder.ui.theme.Fennec

@Composable
fun TemplatesScreen(
    info: DesktopInfo,
    phoneDefault: String?,
    onBack: () -> Unit,
    onSend: () -> Unit,
    onPick: (String?) -> Unit,
) {
    Column(Modifier.fillMaxSize()) {
        Header("Templates", navigation = { HeaderIcon(R.drawable.ic_back, "Back", onBack) })
        if (info.templates.isEmpty()) {
            Empty(
                R.drawable.ic_template, "Send to Fennec once to see its templates",
                "Templates are made in Fennec on your computer: the heading, the fields and how documents look. Each scan brings them to this phone.",
            ) { PrimaryButton("Send to Fennec", onClick = onSend) }
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
