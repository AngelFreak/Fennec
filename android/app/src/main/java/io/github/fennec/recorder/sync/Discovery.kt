package io.github.fennec.recorder.sync

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress
import kotlin.coroutines.resume

/**
 * Finds the paired Fennec on the network by name (mDNS `_fennec._tcp`), for
 * when its address changed since pairing (another Wi-Fi, a new lease).
 * Answers every `host:port` it is announced at, IPv4 first; the caller tries
 * them, since only the certificate pin proves which one is Fennec.
 */
@Suppress("DEPRECATION") // resolveService: the callback form works on every supported version.
suspend fun findFennec(context: Context, id: String, timeoutMs: Long = 6_000): List<String> {
    if (id.isEmpty()) return emptyList()
    val nsd = context.getSystemService(NsdManager::class.java) ?: return emptyList()
    val services = Channel<NsdServiceInfo>(Channel.UNLIMITED)
    val listener = object : NsdManager.DiscoveryListener {
        override fun onServiceFound(info: NsdServiceInfo) { services.trySend(info) }
        override fun onDiscoveryStarted(type: String) {}
        override fun onDiscoveryStopped(type: String) {}
        override fun onServiceLost(info: NsdServiceInfo) {}
        override fun onStartDiscoveryFailed(type: String, error: Int) { services.close() }
        override fun onStopDiscoveryFailed(type: String, error: Int) {}
    }
    val found = mutableListOf<String>()
    try {
        nsd.discoverServices("_fennec._tcp", NsdManager.PROTOCOL_DNS_SD, listener)
        withTimeoutOrNull(timeoutMs) {
            for (service in services) {
                // One at a time: before Android 14 a second resolve fails while one runs.
                val resolved = resolve(nsd, service) ?: continue
                if (resolved.attributes["id"]?.let { String(it) } != id) continue
                found += addresses(resolved).mapNotNull { hostPort(it, resolved.port) }
                if (found.isNotEmpty()) break
            }
        }
    } catch (_: RuntimeException) {
        // NsdManager refuses (busy, or a listener still registered): nothing found this time.
    } finally {
        runCatching { nsd.stopServiceDiscovery(listener) }
        services.close()
    }
    return found.distinct()
}

@Suppress("DEPRECATION")
private suspend fun resolve(nsd: NsdManager, service: NsdServiceInfo): NsdServiceInfo? =
    withTimeoutOrNull(3_000) {
        suspendCancellableCoroutine { cont ->
            nsd.resolveService(service, object : NsdManager.ResolveListener {
                override fun onResolveFailed(info: NsdServiceInfo, error: Int) { if (cont.isActive) cont.resume(null) }
                override fun onServiceResolved(info: NsdServiceInfo) { if (cont.isActive) cont.resume(info) }
            })
        }
    }

@Suppress("DEPRECATION")
private fun addresses(info: NsdServiceInfo): List<InetAddress> {
    val all = if (Build.VERSION.SDK_INT >= 34) info.hostAddresses else listOfNotNull(info.host)
    return all.sortedBy { if (it is Inet4Address) 0 else 1 }
}

/**
 * An address as the client takes it: `192.168.1.20:47130`, or `[2001:db8::5]:47130`.
 * Link-local and loopback addresses are left out: a URL cannot name the
 * Wi-Fi interface a link-local address belongs to, and loopback is the phone.
 */
fun hostPort(a: InetAddress, port: Int): String? = when {
    port !in 1..65535 || a.isLoopbackAddress || a.isLinkLocalAddress || a.isAnyLocalAddress -> null
    a is Inet4Address -> "${a.hostAddress}:$port"
    a is Inet6Address -> "[${a.hostAddress?.substringBefore('%')}]:$port"
    else -> null
}
