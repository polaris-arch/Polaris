// Network registration follows sing-box-for-android's DefaultNetworkListener (GPLv3-or-later).
package com.polaris2.app.vpn

import android.annotation.SuppressLint
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import io.nekohasekai.libbox.InterfaceUpdateListener
import io.nekohasekai.libbox.LocalDNSTransport
import java.net.NetworkInterface
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Each login owns its monitor/DNS supplier. It never replaces or stops the main VPN's listener. */
internal class TransientLoginNetwork(private val requireExactClose: Boolean = false) : PlatformInterfaceWrapper {
    private val callbacks = TransientNetworkCallbacks()
    private val thread = HandlerThread("polaris-login-network").apply { start() }
    private val handler = Handler(thread.looper)
    private val ready = CountDownLatch(1)
    @Volatile private var network: Network? = null
    @Volatile private var listener: InterfaceUpdateListener? = null
    @Volatile private var closed = false
    private var registered = false
    private val resolver = NetworkLocalResolver { network }
    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(value: Network) {
            if (closed || PolarisApplication.connectivity.getNetworkCapabilities(value)
                    ?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN) != true) return
            network = value
            ready.countDown()
            notifyInterface(value)
        }
        override fun onCapabilitiesChanged(value: Network, capabilities: NetworkCapabilities) {
            if (closed) return
            if (capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)) {
                network = value
                ready.countDown()
                notifyInterface(value)
            }
        }
        override fun onLinkPropertiesChanged(value: Network, properties: android.net.LinkProperties) {
            if (!closed && network == value) notifyInterface(value)
        }
        override fun onLost(value: Network) {
            if (network != value) return
            network = null
            notifyInterface(null)
        }
    }

    @SuppressLint("MissingPermission")
    fun start() {
        check(!closed && !registered)
        val request = NetworkRequest.Builder()
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_RESTRICTED)
            .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
            .build()
        val connectivity = PolarisApplication.connectivity
        when {
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
                connectivity.registerBestMatchingNetworkCallback(request, callback, handler)
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.P -> connectivity.requestNetwork(request, callback, handler)
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.O -> connectivity.registerNetworkCallback(request, callback, handler)
            else -> connectivity.registerNetworkCallback(request, callback)
        }
        registered = true
        val active = connectivity.activeNetwork
        if (active != null && connectivity.getNetworkCapabilities(active)
                ?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN) == true) {
            network = active
            ready.countDown()
        }
        check(ready.await(5, TimeUnit.SECONDS) && network != null) { "android: 登录没有可用的物理网络" }
    }

    fun close() {
        if (requireExactClose) callbacks.close { closed = true; listener = null }
        else { closed = true; listener = null }
        try {
            if (registered) {
                try { PolarisApplication.connectivity.unregisterNetworkCallback(callback) }
                catch (_: IllegalArgumentException) { /* The framework already removed this callback. */ }
            }
        } finally {
            registered = false
            network = null
            thread.quitSafely()
            if (requireExactClose) {
                check(Thread.currentThread() !== thread) { "android: network callback cannot confirm its own close" }
                thread.join()
            }
        }
        // Proof unavailability must not mask a real network-unregistration failure.
        if (requireExactClose) resolver.closeUnused()
    }

    override fun sendNotification(notification: io.nekohasekai.libbox.Notification) { /* STATUS is the sole login URL source. */ }
    override fun cancelNotification(identifier: String, typeID: Int) {}
    override fun autoDetectInterfaceControl(fd: Int) = PolarisVpnService.protectTransientSocket(fd)
    override fun localDNSTransport(): LocalDNSTransport = resolver
    override fun startDefaultInterfaceMonitor(value: InterfaceUpdateListener) {
        check(!closed)
        listener = value
        notifyInterface(network)
    }
    override fun closeDefaultInterfaceMonitor(value: InterfaceUpdateListener) {
        // Go Close calls this while joining its native work. It must not wait
        // for a callback that may itself still be inside Go; close() drains it later.
        if (listener == value) listener = null
    }

    private fun notifyInterface(value: Network?) {
        if (requireExactClose) callbacks.dispatch { updateInterface(value) }
        else updateInterface(value)
    }

    private fun updateInterface(value: Network?) {
        val target = listener ?: return
        if (closed) return
        if (value == null) {
            target.updateDefaultInterface("", -1, false, false)
            return
        }
        val name = PolarisApplication.connectivity.getLinkProperties(value)?.interfaceName ?: return
        val index = runCatching { NetworkInterface.getByName(name)?.index }.getOrNull() ?: return
        if (!closed && network == value && listener == target) target.updateDefaultInterface(name, index, false, false)
    }
}

/** Join in-flight native notifications and refuse queued callbacks after close. */
internal class TransientNetworkCallbacks {
    private val gate = Any()
    private var closed = false
    fun dispatch(action: () -> Unit) = synchronized(gate) { if (!closed) action() }
    fun close(action: () -> Unit) = synchronized(gate) { closed = true; action() }
}
