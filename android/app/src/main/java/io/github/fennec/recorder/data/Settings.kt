package io.github.fennec.recorder.data

import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

data class AppSettings(
    /** Send only on networks without a data limit (home Wi-Fi). */
    val unmeteredOnly: Boolean = true,
    /** 48 kHz instead of 16 kHz, for keeping the audio itself. */
    val highQuality: Boolean = true,
    /** Delete a recording from the phone this many days after Fennec has
     *  transcribed it; 0 keeps it. */
    val keepDays: Int = 0,
    /** Template for new recordings when their project has no default;
     *  null leaves it to Fennec. */
    val defaultTemplate: String? = null,
    /** The welcome steps have been seen. */
    val welcomed: Boolean = false,
    /** Keep a copy of recordings not yet sent in Download/Fennec Recorder,
     *  where Fennec finds them over a USB cable. */
    val usbCopies: Boolean = true,
)

class SettingsStore(private val prefs: SharedPreferences) {
    private val _settings = MutableStateFlow(read())
    val settings: StateFlow<AppSettings> = _settings

    private fun read(): AppSettings {
        val d = AppSettings()
        return AppSettings(
            unmeteredOnly = prefs.getBoolean("unmetered_only", d.unmeteredOnly),
            highQuality = prefs.getBoolean("high_quality", d.highQuality),
            keepDays = prefs.getInt("keep_days", d.keepDays),
            defaultTemplate = prefs.getString("default_template", null),
            welcomed = prefs.getBoolean("welcomed", d.welcomed),
            usbCopies = prefs.getBoolean("usb_copies", d.usbCopies),
        )
    }

    fun update(f: (AppSettings) -> AppSettings) {
        val s = f(_settings.value)
        prefs.edit()
            .putBoolean("unmetered_only", s.unmeteredOnly)
            .putBoolean("high_quality", s.highQuality)
            .putInt("keep_days", s.keepDays)
            .putString("default_template", s.defaultTemplate)
            .putBoolean("welcomed", s.welcomed)
            .putBoolean("usb_copies", s.usbCopies)
            .apply()
        _settings.value = s
    }
}
