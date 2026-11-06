package com.hermes.pocketagent.phone

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper
import android.util.Log

/** A PA announcing itself on the LAN (`_pocket-agent._tcp`, see src-tauri/src/api/discovery.rs). */
data class FoundPa(val paId: String, val name: String, val host: String)

/**
 * Browses for PAs with the system NsdManager. [onChange] gets the full list
 * on the main thread whenever it changes. Android 11 resolves one service at
 * a time, so found services queue up for resolving.
 */
class Discovery(private val context: Context, private val onChange: (List<FoundPa>) -> Unit) {
    private val nsd = context.getSystemService(NsdManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    private val found = linkedMapOf<String, FoundPa>() // service name → PA; main thread
    private val toResolve = ArrayDeque<NsdServiceInfo>() // main thread
    private var resolving = false
    private var browsing = false
    // Some Wi-Fi drivers drop multicast unless asked.
    private var multicast: WifiManager.MulticastLock? = null

    private val listener = object : NsdManager.DiscoveryListener {
        override fun onDiscoveryStarted(serviceType: String) {}
        override fun onDiscoveryStopped(serviceType: String) {}
        override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
            Log.w(TAG, "discovery failed: $errorCode")
        }
        override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
        override fun onServiceFound(info: NsdServiceInfo) = main.post {
            toResolve.addLast(info)
            resolveNext()
        }.let {}
        override fun onServiceLost(info: NsdServiceInfo) = main.post {
            if (found.remove(info.serviceName) != null) onChange(found.values.toList())
        }.let {}
    }

    fun start() {
        if (browsing) return
        browsing = true
        multicast = context.getSystemService(WifiManager::class.java)
            .createMulticastLock("PocketAgent:nsd").apply { setReferenceCounted(false); acquire() }
        nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
    }

    fun stop() {
        if (!browsing) return
        browsing = false
        try { nsd.stopServiceDiscovery(listener) } catch (e: IllegalArgumentException) { /* never started */ }
        multicast?.release()
        multicast = null
        toResolve.clear()
    }

    private fun resolveNext() {
        if (resolving || !browsing) return
        val next = toResolve.removeFirstOrNull() ?: return
        resolving = true
        @Suppress("DEPRECATION") // registerServiceInfoCallback is API 34; Pixel 2 is API 30
        nsd.resolveService(next, object : NsdManager.ResolveListener {
            override fun onResolveFailed(info: NsdServiceInfo, errorCode: Int) = main.post {
                resolving = false
                resolveNext()
            }.let {}

            override fun onServiceResolved(info: NsdServiceInfo) = main.post {
                resolving = false
                val attrs = info.attributes
                val id = attrs["id"]?.toString(Charsets.UTF_8).orEmpty()
                val name = attrs["name"]?.toString(Charsets.UTF_8).orEmpty().ifEmpty { info.serviceName }
                @Suppress("DEPRECATION") // hostAddresses is API 34
                val address = info.host?.hostAddress
                if (id.isNotEmpty() && address != null && browsing) {
                    // IPv6 literals need brackets in a URL.
                    val host = if (address.contains(':')) "[$address]:${info.port}" else "$address:${info.port}"
                    found[info.serviceName] = FoundPa(id, name, host)
                    onChange(found.values.toList())
                }
                resolveNext()
            }.let {}
        })
    }

    companion object {
        private const val TAG = "PocketAgent"
        private const val SERVICE_TYPE = "_pocket-agent._tcp"
    }
}
