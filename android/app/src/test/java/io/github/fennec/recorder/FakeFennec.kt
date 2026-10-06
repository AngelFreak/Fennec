package io.github.fennec.recorder

import io.github.fennec.recorder.net.pinOf
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import mockwebserver3.Dispatcher
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.RecordedRequest
import okhttp3.tls.HandshakeCertificates
import okhttp3.tls.HeldCertificate
import java.io.ByteArrayOutputStream
import java.security.MessageDigest

/**
 * Fennec's phone protocol over HTTPS, enough to test the app against: it
 * stores chunks, checks offsets and checksums, and can misbehave on request.
 */
class FakeFennec : Dispatcher() {
    val cert: HeldCertificate = HeldCertificate.Builder().commonName("Fennec").addSubjectAlternativeName("localhost").build()
    val pin: String = pinOf(cert.certificate)
    val server = MockWebServer().apply {
        useHttps(HandshakeCertificates.Builder().heldCertificate(cert).build().sslSocketFactory())
        dispatcher = this@FakeFennec
        // Like Fennec's server: HTTP/1.1 only.
        protocols = listOf(okhttp3.Protocol.HTTP_1_1)
        start()
    }
    val address: String get() = "${server.hostName}:${server.port}"
    val secret = "phone-secret"

    class Rec(val meta: JsonObject) {
        val data = ByteArrayOutputStream()
        var state = "receiving"
        var documentId: Long? = null
    }

    val recordings = mutableMapOf<String, Rec>()
    val requests = mutableListOf<String>()

    /** Lose the answer to this many chunk requests after storing them. */
    @Volatile var loseChunkAnswers = 0

    /** Damage the next upload: flip a byte when it is completed. */
    @Volatile var damageNext = false

    /** This phone was removed in Fennec. */
    @Volatile var unpaired = false

    /** Answer pairing with this, or null to refuse. */
    @Volatile var allowPairing = true

    private val json = Json { ignoreUnknownKeys = true }

    private fun ok(body: String) = MockResponse.Builder().code(200).body(body).build()
    private fun err(code: Int, error: String, received: Long? = null) = MockResponse.Builder().code(code)
        .body("""{"error":"$error","message":"$error message"${received?.let { ",\"received\":$it" } ?: ""}}""").build()

    private fun status(id: String, r: Rec) =
        """{"id":"$id","received":${r.data.size()},"state":"${r.state}","document_id":${r.documentId},"error":null}"""

    @Synchronized
    override fun dispatch(request: RecordedRequest): MockResponse {
        val path = request.url.encodedPath
        requests += "${request.method} $path"
        val parts = path.trim('/').split('/')
        if (request.method == "POST" && path == "/v1/pair") {
            return if (allowPairing) {
                ok("""{"device_id":7,"secret":"$secret","name":"fennec-desktop","id":"abcdefabcdef"}""")
            } else {
                err(403, "denied")
            }
        }
        if (unpaired || request.headers["Authorization"] != "Bearer $secret") return err(401, "not_paired")
        return when {
            request.method == "GET" && path == "/v1/info" ->
                ok("""{"name":"fennec-desktop","projects":[{"id":3,"name":"Kundemøder","color":"#2F6F4E"}],"templates":[{"id":"notat","name":"Notat"}]}""")
            request.method == "PUT" && parts.size == 3 -> {
                val meta = json.parseToJsonElement(request.body!!.utf8()).jsonObject
                val r = recordings.getOrPut(parts[2]) { Rec(meta) }
                ok(status(parts[2], r))
            }
            request.method == "PUT" && parts.size == 4 -> {
                val r = recordings[parts[2]] ?: return err(404, "unknown")
                val offset = request.url.queryParameter("offset")!!.toLong()
                if (offset != r.data.size().toLong()) return err(409, "offset", r.data.size().toLong())
                r.data.write(request.body!!.toByteArray())
                if (loseChunkAnswers > 0) {
                    loseChunkAnswers--
                    return MockResponse.Builder().code(200).onResponseStart(mockwebserver3.SocketEffect.ShutdownConnection).build()
                }
                ok("""{"id":"${parts[2]}","received":${r.data.size()}}""")
            }
            request.method == "POST" && parts.size == 4 && parts[3] == "complete" -> {
                val r = recordings[parts[2]] ?: return err(404, "unknown")
                if (r.state != "receiving") return ok(status(parts[2], r))
                val size = r.meta["size"]!!.jsonPrimitive.long
                if (r.data.size().toLong() != size) return err(409, "incomplete", r.data.size().toLong())
                var bytes = r.data.toByteArray()
                if (damageNext) {
                    damageNext = false
                    bytes = bytes.copyOf().also { it[0] = (it[0] + 1).toByte() }
                }
                val sum = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
                if (sum != r.meta["sha256"]!!.jsonPrimitive.content) {
                    r.data.reset()
                    return err(422, "checksum", 0)
                }
                r.state = "queued"
                r.documentId = 40L + recordings.size
                ok(status(parts[2], r))
            }
            request.method == "GET" && path == "/v1/recordings" -> {
                val ids = request.url.queryParameter("ids")!!.split(',')
                ok("""{"recordings":[${ids.joinToString(",") { id -> recordings[id]?.let { status(id, it) } ?: """{"id":"$id","state":"unknown"}""" }}]}""")
            }
            request.method == "DELETE" && path == "/v1/devices/self" -> ok("{}")
            else -> err(404, "not_found")
        }
    }
}
