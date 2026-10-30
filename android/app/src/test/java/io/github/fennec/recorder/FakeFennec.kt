package io.github.fennec.recorder

import io.github.fennec.recorder.net.SendTarget
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
 * Fennec's phone protocol v2 over HTTPS, enough to test the app against:
 * a single-use ticket per code, a session for the transfer, chunks stored
 * with offsets and checksums checked, and misbehaviour on request.
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

    class Rec(val meta: JsonObject) {
        val data = ByteArrayOutputStream()
        var state = "receiving"
        var documentId: Long? = null
    }

    val recordings = mutableMapOf<String, Rec>()
    val requests = mutableListOf<String>()
    val projects = mutableListOf("""{"id":3,"name":"Kundemøder","color":"#2F6F4E","default_template":null,"documents":4}""")

    /** The ticket in the code shown now. */
    @Volatile var ticket = "AAAAAAAAAAAAAAAAAAAAAA"
        private set
    private var codes = 0

    /** Receive from phone is open (a code is shown). */
    @Volatile var receiving = true
    @Volatile private var ticketUsed = false
    @Volatile var session: String? = null
        private set

    /** Phones that said hello, by name. */
    val phones = mutableListOf<String>()
    @Volatile var doneCalls = 0

    /** Lose the answer to this many chunk requests after storing them. */
    @Volatile var loseChunkAnswers = 0

    /** Damage the next upload: flip a byte when it is completed. */
    @Volatile var damageNext = false

    /** End the session (Stop in Fennec) after this many more chunks. */
    @Volatile var stopAfterChunks = Int.MAX_VALUE

    /** Refuse announcing recordings with this title, for good. */
    @Volatile var refuseTitle: String? = null

    /** Wait this long before answering each chunk. */
    @Volatile var chunkDelayMs = 0L

    private val json = Json { ignoreUnknownKeys = true }

    /** The code Fennec shows now, as the phone reads it. */
    fun code() = SendTarget(address, pin, ticket, "fennec-desktop")

    /** Receive from phone again: a new code, the old session gone. */
    @Synchronized
    fun newCode() {
        ticket = "BBBBBBBBBBBBBBBBBBBB" + "%02d".format(++codes)
        ticketUsed = false
        receiving = true
        session = null
    }

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
        if (request.method == "POST" && path == "/v1/pair") return err(410, "update_app")
        if (request.method == "POST" && path == "/v1/hello") {
            val v = json.parseToJsonElement(request.body!!.utf8()).jsonObject
            if (!receiving) return err(403, "no_ticket")
            if (ticketUsed) return err(403, "used")
            if (v["ticket"]?.jsonPrimitive?.content != ticket) return err(403, "wrong_ticket")
            ticketUsed = true
            session = "session-$ticket"
            phones += v["device_name"]!!.jsonPrimitive.content
            return ok(
                """{"session":"$session","name":"fennec-desktop","id":"abcdefabcdef","version":"0.9.0","protocol":2,""" +
                    """"projects":[${projects.joinToString(",")}],""" +
                    """"templates":[{"id":"notat","name":"Notat","fields":[{"key":"sagsnr","label":"Sagsnr.","kind":"text","required":true}]}],""" +
                    """"default_template":"notat","project_colors":["#C2410C","#1D4ED8"]}""",
            )
        }
        val s = session
        if (s == null || request.headers["Authorization"] != "Bearer $s") return err(401, "no_session")
        return when {
            request.method == "POST" && path == "/v1/done" -> {
                doneCalls++
                session = null
                ok("{}")
            }
            request.method == "PUT" && parts.size == 3 -> {
                val meta = json.parseToJsonElement(request.body!!.utf8()).jsonObject
                if (meta["title"]?.jsonPrimitive?.content == refuseTitle) return err(400, "too_long")
                val r = recordings.getOrPut(parts[2]) { Rec(meta) }
                ok(status(parts[2], r))
            }
            request.method == "PUT" && parts.size == 4 -> {
                val r = recordings[parts[2]] ?: return err(404, "unknown")
                val offset = request.url.queryParameter("offset")!!.toLong()
                if (offset != r.data.size().toLong()) return err(409, "offset", r.data.size().toLong())
                if (chunkDelayMs > 0) Thread.sleep(chunkDelayMs)
                r.data.write(request.body!!.toByteArray())
                if (--stopAfterChunks <= 0) session = null
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
            else -> err(404, "not_found")
        }
    }
}
