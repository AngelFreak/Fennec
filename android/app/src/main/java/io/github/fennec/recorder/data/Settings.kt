package io.github.fennec.recorder.data

import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

data class AppSettings(
    /** Send only on networks without a data limit (home Wi-Fi). */
    val unmeteredOnly: Boolean = true,
    /** 48 kHz instead of 16 kHz, for people who want the audio itself. */
    val highQuality: Boolean = false,
    /** Delete a recording from the phone this many days after Fennec has
     *  transcribed it; 0 keeps it. */
    val keepDays: Int = 30,
)

class SettingsStore(private val prefs: SharedPreferences) {
    private val _settings = MutableStateFlow(read())
    val settings: StateFlow<AppSettings> = _settings

    private fun read() = AppSettings(
        unmeteredOnly = prefs.getBoolean("unmetered_only", true),
        highQuality = prefs.getBoolean("high_quality", false),
        keepDays = prefs.getInt("keep_days", 30),
    )

    fun update(f: (AppSettings) -> AppSettings) {
        val s = f(_settings.value)
        prefs.edit()
            .putBoolean("unmetered_only", s.unmeteredOnly)
            .putBoolean("high_quality", s.highQuality)
            .putInt("keep_days", s.keepDays)
            .apply()
        _settings.value = s
    }
}
