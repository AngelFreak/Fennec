package io.github.fennec.recorder.net

import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.Base64
import javax.net.ssl.X509TrustManager

/** SHA-256 of a certificate's public key (SubjectPublicKeyInfo), unpadded URL-safe base64. */
fun pinOf(cert: X509Certificate): String =
    Base64.getUrlEncoder().withoutPadding()
        .encodeToString(MessageDigest.getInstance("SHA-256").digest(cert.publicKey.encoded))

/**
 * Accepts exactly one certificate: the one whose public key has [expected]
 * as its pin. Fennec's certificate has no authority behind it; the pin from
 * the QR code is what proves it. With no pin yet (an address typed in), the
 * first certificate seen is accepted and its pin reported through [seen].
 */
class PinningTrustManager(
    private val expected: String?,
    private val seen: (String) -> Unit = {},
) : X509TrustManager {
    override fun checkServerTrusted(chain: Array<out X509Certificate>, authType: String) {
        val leaf = chain.firstOrNull() ?: throw CertificateException("no certificate")
        val pin = pinOf(leaf)
        if (expected != null && pin != expected) throw PinMismatch(pin)
        seen(pin)
    }

    override fun checkClientTrusted(chain: Array<out X509Certificate>, authType: String) =
        throw CertificateException("not a server")

    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
}

class PinMismatch(val pin: String) : CertificateException("This is not the Fennec this phone was paired with")
