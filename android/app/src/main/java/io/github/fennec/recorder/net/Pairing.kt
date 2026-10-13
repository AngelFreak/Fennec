package io.github.fennec.recorder.net

import java.net.URI
import java.net.URLDecoder
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64

/** A Fennec to pair with: from its QR code, or typed in. */
data class PairingTarget(
    /** `host:port`. */
    val address: String,
    /** 8 digits. */
    val token: String,
    /** Certificate pin from the QR code; `null` when typed in (trusted on first use). */
    val pin: String?,
    /** The computer's name, if the QR code gave it. */
    val name: String?,
)

object PairingUri {
    /** `fennec://pair?h=<ip:port>&pin=<pin>&t=<token>&n=<name>`, as Fennec's QR code holds. */
    fun parse(text: String): PairingTarget? {
        val uri = runCatching { URI(text.trim()) }.getOrNull() ?: return null
        if (uri.scheme != "fennec" || (uri.host ?: uri.schemeSpecificPart.substringBefore('?').trim('/')) != "pair") {
            return null
        }
        val query = uri.rawQuery ?: return null
        val params = query.split('&').mapNotNull {
            val (k, v) = it.split('=', limit = 2).takeIf { p -> p.size == 2 } ?: return@mapNotNull null
            k to URLDecoder.decode(v, "UTF-8")
        }.toMap()
        val address = params["h"]?.takeIf(::validAddress) ?: return null
        val token = params["t"]?.let(::normalizeToken) ?: return null
        val pin = params["pin"]?.takeIf { it.matches(Regex("[A-Za-z0-9_-]{43}")) } ?: return null
        return PairingTarget(address, token, pin, params["n"]?.takeIf { it.isNotBlank() })
    }

    /** Typed in: an address (port optional) and the code Fennec shows. */
    fun manual(address: String, code: String, defaultPort: Int = DEFAULT_PORT): PairingTarget? {
        val a = address.trim().let { if (':' in it) it else "$it:$defaultPort" }
        if (!validAddress(a)) return null
        val token = normalizeToken(code) ?: return null
        return PairingTarget(a, token, null, null)
    }

    fun normalizeToken(code: String): String? = code.filterNot(Char::isWhitespace).takeIf { it.matches(Regex("\\d{8}")) }

    private fun validAddress(a: String): Boolean {
        val host = a.substringBeforeLast(':')
        val port = a.substringAfterLast(':').toIntOrNull() ?: return false
        return host.isNotBlank() && port in 1..65535 && host.all { it.isLetterOrDigit() || it in ".-[]:" }
    }

    const val DEFAULT_PORT = 47130
}

object PairingCode {
    /**
     * The code both screens show: the first 8 bytes of SHA-256 of
     * `pin|token|nonce` as a big-endian number, modulo 10^8 (same as Fennec's
     * `sync::pairing_code`).
     */
    fun of(pin: String, token: String, nonce: String): String {
        val d = MessageDigest.getInstance("SHA-256").digest("$pin|$token|$nonce".toByteArray())
        var n = 0UL
        for (i in 0 until 8) n = (n shl 8) or (d[i].toULong() and 0xFFUL)
        val digits = (n % 100_000_000UL).toString().padStart(8, '0')
        return "${digits.substring(0, 4)} ${digits.substring(4)}"
    }

    fun nonce(): String {
        val b = ByteArray(18).also { SecureRandom().nextBytes(it) }
        return Base64.getUrlEncoder().withoutPadding().encodeToString(b)
    }
}
