package io.github.fennec.recorder.sync

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Finds the paired Fennec on the network by name (mDNS `_fennec._tcp`), for
 * when its address changed since pairing. Answers `host:port`, or null.
 */
@Suppress("DEPRECATION") // resolveService: the callback form works on every supported version.
suspend fun findFennec(context: Context, id: String, timeoutMs: Long = 5_000): String? {
    if (id.isEmpty()) return null
    val nsd = context.getSystemService(NsdManager::class.java) ?: return null
    val found = CompletableDeferred<String?>()
    val listener = object : NsdManager.DiscoveryListener {
        override fun onServiceFound(info: NsdServiceInfo) {
            nsd.resolveService(info, object : NsdManager.ResolveListener {
                override fun onResolveFailed(info: NsdServiceInfo, error: Int) {}
                override fun onServiceResolved(info: NsdServiceInfo) {
                    val theirs = info.attributes["id"]?.let { String(it) }
                    val host = info.host?.hostAddress
                    if (theirs == id && host != null) found.complete("$host:${info.port}")
                }
            })
        }
        override fun onDiscoveryStarted(type: String) {}
        override fun onDiscoveryStopped(type: String) {}
        override fun onServiceLost(info: NsdServiceInfo) {}
        override fun onStartDiscoveryFailed(type: String, error: Int) { found.complete(null) }
        override fun onStopDiscoveryFailed(type: String, error: Int) {}
    }
    return try {
        nsd.discoverServices("_fennec._tcp", NsdManager.PROTOCOL_DNS_SD, listener)
        withTimeoutOrNull(timeoutMs) { found.await() }
    } finally {
        runCatching { nsd.stopServiceDiscovery(listener) }
    }
}
