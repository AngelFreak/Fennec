package io.github.fennec.recorder.data

import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** The computer this phone sends to. */
data class Paired(
    /** The computer's name. */
    val name: String,
    /** Which Fennec it is (stable across address changes). */
    val id: String,
    /** `host:port` it was last reached at. */
    val address: String,
    /** Certificate pin; no other certificate is accepted. */
    val pin: String,
    val deviceId: Long,
    val secret: String,
)

/** Keeps the device secret out of plain storage. */
interface SecretCipher {
    fun seal(plain: String): String
    fun open(sealed: String): String
}

/** AES-GCM with a key that never leaves the Android Keystore. */
class KeystoreCipher : SecretCipher {
    private val alias = "fennec-device-secret"

    private fun key(): SecretKey {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (ks.getEntry(alias, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        gen.init(
            KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .build(),
        )
        return gen.generateKey()
    }

    override fun seal(plain: String): String {
        val c = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key()) }
        val out = c.iv + c.doFinal(plain.toByteArray())
        return Base64.encodeToString(out, Base64.NO_WRAP)
    }

    override fun open(sealed: String): String {
        val raw = Base64.decode(sealed, Base64.NO_WRAP)
        val c = Cipher.getInstance("AES/GCM/NoPadding")
        c.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, raw, 0, 12))
        return String(c.doFinal(raw, 12, raw.size - 12))
    }
}

@Serializable
data class Project(val id: Long, val name: String, val color: String = "#9AA1AE")

@Serializable
data class Template(val id: String, val name: String)

/** What Fennec offers to file recordings under, as last read from it. */
@Serializable
data class DesktopInfo(val projects: List<Project> = emptyList(), val templates: List<Template> = emptyList())

class PairingStore(private val prefs: SharedPreferences, private val cipher: SecretCipher) {
    private val json = Json { ignoreUnknownKeys = true }
    private val _paired = MutableStateFlow(read())
    val paired: StateFlow<Paired?> = _paired
    private val _info = MutableStateFlow(readInfo())
    val info: StateFlow<DesktopInfo> = _info

    private fun read(): Paired? {
        val sealed = prefs.getString("secret", null) ?: return null
        val secret = runCatching { cipher.open(sealed) }.getOrNull() ?: return null
        return Paired(
            name = prefs.getString("name", null) ?: return null,
            id = prefs.getString("id", "") ?: "",
            address = prefs.getString("address", null) ?: return null,
            pin = prefs.getString("pin", null) ?: return null,
            deviceId = prefs.getLong("device_id", 0),
            secret = secret,
        )
    }

    private fun readInfo(): DesktopInfo =
        prefs.getString("info", null)?.let { runCatching { json.decodeFromString<DesktopInfo>(it) }.getOrNull() }
            ?: DesktopInfo()

    fun save(p: Paired) {
        prefs.edit()
            .putString("name", p.name).putString("id", p.id).putString("address", p.address)
            .putString("pin", p.pin).putLong("device_id", p.deviceId).putString("secret", cipher.seal(p.secret))
            .apply()
        _paired.value = p
    }

    /** Fennec moved to another address on the network (found by name). */
    fun moved(address: String) {
        val p = _paired.value ?: return
        if (p.address == address) return
        prefs.edit().putString("address", address).apply()
        _paired.value = p.copy(address = address)
    }

    fun saveInfo(info: DesktopInfo) {
        prefs.edit().putString("info", json.encodeToString(DesktopInfo.serializer(), info)).apply()
        _info.value = info
    }

    fun forget() {
        prefs.edit().clear().apply()
        _paired.value = null
        _info.value = DesktopInfo()
    }
}
