package io.github.fennec.recorder.data

import android.annotation.SuppressLint
import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.security.KeyStore

@Serializable
data class Project(
    val id: Long,
    val name: String,
    val color: String = "#9AA1AE",
    /** The template new documents in it get, unless one is chosen. */
    @SerialName("default_template") val defaultTemplate: String? = null,
    val documents: Int = 0,
)

/** A header field of a template (filled in on the computer). */
@Serializable
data class TemplateField(val key: String, val label: String, val kind: String = "text", val required: Boolean = false)

@Serializable
data class Template(val id: String, val name: String, val fields: List<TemplateField> = emptyList())

/** What Fennec offers to file recordings under, as last read from it. */
@Serializable
data class DesktopInfo(
    val projects: List<Project> = emptyList(),
    val templates: List<Template> = emptyList(),
    /** Fennec's own default template. */
    @SerialName("default_template") val defaultTemplate: String? = null,
    /** Colours a project can have, as Fennec's sidebar offers them. */
    @SerialName("project_colors") val projectColors: List<String> = DEFAULT_COLORS,
) {
    fun template(id: String?): Template? = id?.let { t -> templates.firstOrNull { it.id == t } }
    fun project(id: Long?): Project? = id?.let { p -> projects.firstOrNull { it.id == p } }

    /** Nothing read from Fennec yet: the pickers have only Unsorted and the default. */
    val empty: Boolean get() = projects.isEmpty() && templates.isEmpty()

    companion object {
        val DEFAULT_COLORS = listOf("#C2410C", "#1D4ED8", "#0F766E", "#6B21A8", "#9AA1AE")
    }
}

/**
 * Fennec's projects and templates and the computer's name, as the last
 * scan read them, for the pickers. Nothing secret: sending needs a new code.
 */
class DesktopStore(private val prefs: SharedPreferences) {
    private val json = Json { ignoreUnknownKeys = true }
    private val _info = MutableStateFlow(readInfo())
    val info: StateFlow<DesktopInfo> = _info
    private val _name = MutableStateFlow(prefs.getString("name", null))

    /** The computer last sent to. */
    val name: StateFlow<String?> = _name

    private fun readInfo(): DesktopInfo = decode(prefs.getString("info", null)) ?: DesktopInfo()

    private fun decode(text: String?): DesktopInfo? =
        text?.let { runCatching { json.decodeFromString<DesktopInfo>(it) }.getOrNull() }

    fun save(name: String, info: DesktopInfo) {
        prefs.edit().putString("name", name).putString("info", json.encodeToString(DesktopInfo.serializer(), info)).apply()
        _name.value = name
        _info.value = info
    }

    /**
     * Before sending by QR code (protocol v1) the phone was paired: it kept
     * Fennec's address, certificate pin and a device secret sealed with a
     * Keystore key. Forgets all of that once, keeping only the projects and
     * templates it had read.
     */
    @SuppressLint("ApplySharedPref") // Cleared on disk before its file is deleted.
    fun forgetOldPairing(context: Context, deleteKey: () -> Unit = ::deleteOldKey) {
        if (prefs.getBoolean("old_pairing_forgotten", false)) return
        val old = context.getSharedPreferences(OLD_PAIRING, Context.MODE_PRIVATE)
        if (prefs.getString("info", null) == null) {
            decode(old.getString("info", null))?.let { info ->
                val name = old.getString("name", null)
                val edit = prefs.edit().putString("info", json.encodeToString(DesktopInfo.serializer(), info))
                if (name != null) edit.putString("name", name)
                edit.apply()
                _info.value = info
                _name.value = name
            }
        }
        old.edit().clear().commit()
        context.deleteSharedPreferences(OLD_PAIRING)
        runCatching(deleteKey)
        prefs.edit().putBoolean("old_pairing_forgotten", true).apply()
    }

    companion object {
        private const val OLD_PAIRING = "pairing"

        /** The Keystore key the old device secret was sealed with. */
        const val OLD_KEY_ALIAS = "fennec-device-secret"

        fun deleteOldKey() {
            val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            if (ks.containsAlias(OLD_KEY_ALIAS)) ks.deleteEntry(OLD_KEY_ALIAS)
        }
    }
}
