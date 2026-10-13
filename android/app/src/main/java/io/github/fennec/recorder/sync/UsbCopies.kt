package io.github.fennec.recorder.sync

import android.content.ContentResolver
import android.content.ContentValues
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import io.github.fennec.recorder.data.Recording
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.File
import java.io.OutputStream

/** The folder Fennec looks in over USB, under the phone's shared storage. */
val USB_FOLDER = Environment.DIRECTORY_DOWNLOADS + "/Fennec Recorder"

/**
 * Copies of recordings in shared storage for Fennec to import over a USB
 * cable. Android shows an app only the files it made, so the app cannot
 * read a reply from Fennec; Fennec deletes the copy instead, and the app
 * checks whether its copy still exists.
 */
interface SharedCopies {
    /** Writes the audio, then the sidecar (Fennec imports only with both). Their URIs, or null. */
    fun export(audio: File, name: String, sidecar: String): Pair<String, String>?

    /** Replaces a sidecar (the title or project changed). Its new URI. */
    fun replaceSidecar(old: String?, name: String, sidecar: String): String?

    fun exists(uri: String): Boolean

    fun delete(uri: String)
}

/** The sidecar `<id>.json`, as Fennec's `sync::usb::Sidecar` reads it. */
fun usbSidecar(r: Recording, deviceId: Long?): String = buildJsonObject {
    put("fennec_recorder", 1)
    put("id", r.id)
    put("title", r.title)
    put("recorded_at", r.recordedAt)
    put("duration_ms", r.durationMs)
    put("project_id", r.projectId)
    put("template_id", r.templateId)
    put("ext", r.ext)
    put("size", r.size)
    put("sha256", r.sha256)
    put("device_id", deviceId)
}.toString()

/** Shared copies through MediaStore's Downloads collection (no permission needed). */
class MediaStoreCopies(private val resolver: ContentResolver) : SharedCopies {
    private val collection: Uri = MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)

    private fun put(name: String, mime: String, write: (OutputStream) -> Unit): String? {
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(MediaStore.MediaColumns.MIME_TYPE, mime)
            put(MediaStore.MediaColumns.RELATIVE_PATH, USB_FOLDER)
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = runCatching { resolver.insert(collection, values) }.getOrNull() ?: return null
        val ok = runCatching { resolver.openOutputStream(uri)?.use(write) != null }.getOrDefault(false)
        if (!ok) {
            runCatching { resolver.delete(uri, null, null) }
            return null
        }
        resolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
        return uri.toString()
    }

    override fun export(audio: File, name: String, sidecar: String): Pair<String, String>? {
        val a = put("$name.${audio.extension}", "audio/aac") { out -> audio.inputStream().use { it.copyTo(out) } }
            ?: return null
        val m = put("$name.json", "application/json") { it.write(sidecar.toByteArray()) }
        if (m == null) {
            delete(a)
            return null
        }
        return a to m
    }

    override fun replaceSidecar(old: String?, name: String, sidecar: String): String? {
        old?.let(::delete)
        return put("$name.json", "application/json") { it.write(sidecar.toByteArray()) }
    }

    override fun exists(uri: String): Boolean = runCatching {
        resolver.query(Uri.parse(uri), arrayOf(MediaStore.MediaColumns._ID), null, null, null)?.use { it.moveToFirst() }
    }.getOrNull() ?: false

    override fun delete(uri: String) {
        runCatching { resolver.delete(Uri.parse(uri), null, null) }
    }
}
