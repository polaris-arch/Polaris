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
internal class TransientLoginNetwork : PlatformInterfaceWrapper {
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
        closed = true
        listener = null
        try {
            if (registered) {
                try { PolarisApplication.connectivity.unregisterNetworkCallback(callback) }
                catch (_: IllegalArgumentException) { /* The framework already removed this callback. */ }
            }
        } finally {
            registered = false
            network = null
            thread.quitSafely()
        }
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
        if (listener == value) listener = null
    }

    private fun notifyInterface(value: Network?) {
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
