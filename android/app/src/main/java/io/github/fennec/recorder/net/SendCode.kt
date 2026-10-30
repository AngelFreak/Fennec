package io.github.fennec.recorder.net

import java.net.URLDecoder

/** A Fennec ready to receive, from the QR code it shows for one transfer. */
data class SendTarget(
    /** `host:port`. */
    val address: String,
    /** Certificate pin; no other certificate is accepted. */
    val pin: String,
    /** Single-use ticket that opens the transfer. */
    val ticket: String,
    /** The computer's name, if the code gave it. */
    val name: String?,
)

/** What a scanned code or opened link is. */
sealed interface ScannedCode {
    data class Send(val target: SendTarget) : ScannedCode

    /** A pairing code from a Fennec before sending by QR code. */
    data object OldFennec : ScannedCode

    /** Not a code from Fennec, or a damaged one. */
    data object Other : ScannedCode
}

object SendUri {
    /** `fennec://send?h=<ip:port>&pin=<pin>&t=<ticket>&n=<name>`, as Fennec's QR code holds. */
    fun read(text: String): ScannedCode {
        val t = text.trim()
        // Read by hand: java.net.URI refuses an IPv6 address's brackets in a query.
        if (!t.startsWith("fennec:", ignoreCase = true)) return ScannedCode.Other
        val rest = t.substringAfter(':').trimStart('/')
        val kind = rest.substringBefore('?').trimEnd('/')
        if (kind == "pair") return ScannedCode.OldFennec
        if (kind != "send" || '?' !in rest) return ScannedCode.Other
        val params = rest.substringAfter('?').substringBefore('#').split('&').mapNotNull {
            val (k, v) = it.split('=', limit = 2).takeIf { p -> p.size == 2 } ?: return@mapNotNull null
            k to runCatching { URLDecoder.decode(v, "UTF-8") }.getOrNull()
        }.toMap()
        val address = params["h"]?.takeIf(::validAddress) ?: return ScannedCode.Other
        val pin = params["pin"]?.takeIf { it.matches(Regex("[A-Za-z0-9_-]{43}")) } ?: return ScannedCode.Other
        val ticket = params["t"]?.takeIf { it.matches(Regex("[A-Za-z0-9_-]{16,128}")) } ?: return ScannedCode.Other
        return ScannedCode.Send(SendTarget(address, pin, ticket, params["n"]?.takeIf { it.isNotBlank() }))
    }

    /** The target, or null for anything else. */
    fun parse(text: String): SendTarget? = (read(text) as? ScannedCode.Send)?.target

    private fun validAddress(a: String): Boolean {
        val host = a.substringBeforeLast(':')
        val port = a.substringAfterLast(':').toIntOrNull() ?: return false
        return host.isNotBlank() && port in 1..65535 && host.all { it.isLetterOrDigit() || it in ".-[]:" }
    }
}
