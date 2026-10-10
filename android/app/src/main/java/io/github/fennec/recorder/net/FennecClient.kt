package io.github.fennec.recorder.net

import io.github.fennec.recorder.data.DesktopInfo
import io.github.fennec.recorder.data.Project
import io.github.fennec.recorder.data.Template
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.IOException
import java.security.SecureRandom
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLHandshakeException

/** An answer from Fennec, or why there is none. */
sealed interface Api<out T> {
    data class Ok<T>(val value: T) : Api<T>

    /** Fennec answered with an error: `code` and `message` from its JSON. */
    data class Refused(val status: Int, val code: String, val message: String, val received: Long?) : Api<Nothing>

    /** Fennec could not be reached (or another computer answered). */
    data class Unreachable(val cause: Throwable) : Api<Nothing> {
        /** A pin mismatch anywhere in the failure: as its cause, or suppressed
         *  under a later attempt (OkHttp tries each address of a host). */
        val wrongComputer: Boolean get() = involves(cause, mutableSetOf())

        private fun involves(e: Throwable?, seen: MutableSet<Throwable>): Boolean {
            if (e == null || !seen.add(e)) return false
            return e is PinMismatch || involves(e.cause, seen) || e.suppressed.any { involves(it, seen) }
        }
    }
}

/** Where a recording is, as Fennec reports it. */
data class RemoteStatus(
    val id: String,
    val received: Long,
    val state: String,
    val documentId: Long?,
    val error: String?,
)

data class PairResult(val deviceId: Long, val secret: String, val name: String, val id: String)

/** What the phone tells Fennec before sending a recording. */
data class Announcement(
    val id: String,
    val title: String,
    val recordedAt: Long,
    val durationMs: Long,
    val projectId: Long?,
    val templateId: String?,
    val ext: String,
    val size: Long,
    val sha256: String,
)

/**
 * Fennec's phone protocol (see the desktop's
 * `docs/plans/2026-10-06-android-recorder.md`). Every call goes to [address]
 * and accepts only the certificate with [pin].
 */
class FennecClient(
    val address: String,
    pin: String?,
    private val secret: String? = null,
    /** The pin of the certificate seen, when none was known yet. */
    onPin: (String) -> Unit = {},
    timeoutSeconds: Long = 30,
) {
    private val json = Json { ignoreUnknownKeys = true }
    private val http: OkHttpClient = run {
        val trust = PinningTrustManager(pin, onPin)
        val tls = SSLContext.getInstance("TLS").apply { init(null, arrayOf(trust), SecureRandom()) }
        OkHttpClient.Builder()
            .sslSocketFactory(tls.socketFactory, trust)
            // The pin identifies Fennec; its certificate names no address.
            .hostnameVerifier { _, _ -> true }
            .connectTimeout(10, TimeUnit.SECONDS)
            .readTimeout(timeoutSeconds, TimeUnit.SECONDS)
            .writeTimeout(timeoutSeconds, TimeUnit.SECONDS)
            .retryOnConnectionFailure(false)
            .build()
    }
    private val jsonType = "application/json".toMediaType()
    private val octets = "application/octet-stream".toMediaType()

    private suspend fun call(method: String, path: String, body: RequestBody? = null): Api<JsonObject> =
        withContext(Dispatchers.IO) {
            val req = Request.Builder().url("https://$address$path").method(method, body)
                .apply { secret?.let { header("Authorization", "Bearer $it") } }
                .build()
            try {
                http.newCall(req).execute().use { resp ->
                    val text = resp.body.string()
                    val obj = runCatching { json.parseToJsonElement(text).jsonObject }.getOrDefault(JsonObject(emptyMap()))
                    if (resp.isSuccessful) {
                        Api.Ok(obj)
                    } else {
                        Api.Refused(
                            resp.code,
                            obj.str("error") ?: "http_${resp.code}",
                            obj.str("message") ?: "Fennec answered ${resp.code}.",
                            obj["received"]?.jsonPrimitive?.longOrNull,
                        )
                    }
                }
            } catch (e: SSLHandshakeException) {
                Api.Unreachable(e)
            } catch (e: IOException) {
                Api.Unreachable(e)
            }
        }

    private fun <A, B> Api<A>.map(f: (A) -> B): Api<B> = when (this) {
        is Api.Ok -> Api.Ok(f(value))
        is Api.Refused -> this
        is Api.Unreachable -> this
    }

    private fun JsonObject.str(k: String) = this[k]?.takeIf { it !is JsonNull }?.jsonPrimitive?.contentOrNull

    private fun status(o: JsonObject) = RemoteStatus(
        id = o.str("id") ?: "",
        received = o["received"]?.jsonPrimitive?.longOrNull ?: 0,
        state = o.str("state") ?: "unknown",
        documentId = o["document_id"]?.takeIf { it !is JsonNull }?.jsonPrimitive?.longOrNull,
        error = o.str("error"),
    )

    private fun JsonElement.body() = toString().toRequestBody(jsonType)

    suspend fun pair(token: String, deviceName: String, nonce: String): Api<PairResult> =
        call("POST", "/v1/pair", buildJsonObject {
            put("token", token); put("device_name", deviceName); put("nonce", nonce)
        }.body()).map {
            PairResult(
                it["device_id"]?.jsonPrimitive?.longOrNull ?: 0,
                it.str("secret") ?: "",
                it.str("name") ?: "Fennec",
                it.str("id") ?: "",
            )
        }

    suspend fun info(): Api<DesktopInfo> = call("GET", "/v1/info").map { json.decodeFromJsonElement<DesktopInfo>(it) }

    suspend fun createProject(name: String, color: String, defaultTemplate: String?): Api<Project> =
        call("POST", "/v1/projects", buildJsonObject {
            put("name", name); put("color", color)
            defaultTemplate?.let { put("default_template", it) }
        }.body()).map { json.decodeFromJsonElement<Project>(it) }

    /** Changes name, colour and default template (null: Fennec's default). */
    suspend fun updateProject(id: Long, name: String, color: String, defaultTemplate: String?): Api<Project> =
        call("PUT", "/v1/projects/$id", buildJsonObject {
            put("name", name); put("color", color); put("default_template", defaultTemplate)
        }.body()).map { json.decodeFromJsonElement<Project>(it) }

    suspend fun announce(a: Announcement): Api<RemoteStatus> =
        call("PUT", "/v1/recordings/${a.id}", buildJsonObject {
            put("title", a.title); put("recorded_at", a.recordedAt); put("duration_ms", a.durationMs)
            a.projectId?.let { put("project_id", it) }
            a.templateId?.let { put("template_id", it) }
            put("ext", a.ext); put("size", a.size); put("sha256", a.sha256)
        }.body()).map(::status)

    /** One chunk; answers how many bytes Fennec has. */
    suspend fun chunk(id: String, offset: Long, bytes: ByteArray): Api<Long> =
        call("PUT", "/v1/recordings/$id/audio?offset=$offset", bytes.toRequestBody(octets)).map {
            it["received"]?.jsonPrimitive?.longOrNull ?: (offset + bytes.size)
        }

    suspend fun complete(id: String): Api<RemoteStatus> =
        call("POST", "/v1/recordings/$id/complete", ByteArray(0).toRequestBody(octets)).map(::status)

    suspend fun statuses(ids: List<String>): Api<List<RemoteStatus>> =
        call("GET", "/v1/recordings?ids=${ids.joinToString(",")}").map {
            it["recordings"]?.jsonArray?.map { s -> status(s.jsonObject) }.orEmpty()
        }

    suspend fun unpair(): Api<Unit> = call("DELETE", "/v1/devices/self").map { }

    companion object {
        /** Pairing waits for someone to press Allow in Fennec (up to 120 s). */
        const val PAIR_TIMEOUT_SECONDS = 150L
    }
}
