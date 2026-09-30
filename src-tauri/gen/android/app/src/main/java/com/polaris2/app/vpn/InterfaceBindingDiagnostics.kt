package com.polaris2.app.vpn

import android.net.NetworkCapabilities
import android.os.ParcelFileDescriptor
import android.os.Process
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import com.polaris2.app.BuildConfig
import java.io.File
import java.io.FileDescriptor
import org.json.JSONArray
import org.json.JSONObject

/** Fresh local sockets only: never connect/send, register a network, or change process routing. */
internal object InterfaceBindingDiagnostics {
    fun collect(): String {
        check(BuildConfig.DEBUG)
        val rows = JSONArray()
        val names = InterfaceBoundSocket.interfaces().filter { it.isUp }.map { it.name }.take(8)
        for (name in names) for (type in listOf(OsConstants.SOCK_STREAM, OsConstants.SOCK_DGRAM)) {
            rows.put(measure(name, type, true))
            rows.put(measure(name, type, false))
        }
        for (type in listOf(OsConstants.SOCK_STREAM, OsConstants.SOCK_DGRAM)) {
            rows.put(measure("polaris-invalid-probe", type, false))
        }
        val observedNetworks = JSONArray()
        @Suppress("DEPRECATION")
        for (network in PolarisApplication.connectivity.allNetworks) {
            val caps = PolarisApplication.connectivity.getNetworkCapabilities(network) ?: continue
            val name = PolarisApplication.connectivity.getLinkProperties(network)?.interfaceName ?: continue
            observedNetworks.put(JSONObject().put("interface", name)
                .put("wifi", caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI))
                .put("cellular", caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR))
                .put("vpn", caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN))
                .put("notVpn", caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN))
                .put("notRestricted", caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_RESTRICTED))
                .put("internet", caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET))
                .put("ims", caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_IMS))
                .put("mms", caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_MMS))
                .put("validated", caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)))
        }
        val domain = runCatching { File("/proc/self/attr/current").readText().trim().split(':').getOrNull(2) }
            .getOrNull()
        return JSONObject().put("uid", Process.myUid()).put("seLinuxDomain", domain)
            .put("vpnServicePresent", PolarisVpnService.hasSocketProtector())
            .put("noNetworkTraffic", true).put("observedNetworks", observedNetworks).put("rows", rows).toString()
    }

    private fun measure(name: String, type: Int, raw: Boolean): JSONObject {
        val result = JSONObject().put("interface", name)
            .put("transport", if (type == OsConstants.SOCK_STREAM) "tcp" else "udp")
            .put("method", if (raw) "raw_bindtoifindex" else "network_bindSocket")
        var socket: FileDescriptor? = null
        var holder: ParcelFileDescriptor? = null
        var borrowed: FileDescriptor? = null
        try {
            socket = Os.socket(OsConstants.AF_INET, type, 0)
            holder = ParcelFileDescriptor.dup(socket)
            if (raw) {
                val index = Os.if_nametoindex(name)
                if (index == 0) throw InterfaceBindingFailure("BIND_INTERFACE_UNAVAILABLE")
                // Same first syscall as sing/common/control/bind_linux.go; no fallback is inferred here.
                Os.setsockoptInt(holder.fileDescriptor, OsConstants.SOL_SOCKET, 62, index)
            } else InterfaceBoundSocket.bind(holder.fd, name) { borrowed = it }
            result.put("success", true)
        } catch (error: Exception) {
            result.put("success", false).put("errorCode", when (error) {
                is InterfaceBindingFailure -> error.code
                is ErrnoException -> "ERRNO_${error.errno}"
                else -> error.javaClass.simpleName
            })
        } finally {
            result.put("originalFdAlive", socket?.valid() == true)
            result.put("callerFdAlive", holder?.fileDescriptor?.valid() == true)
            result.put("borrowedDuplicateClosed", borrowed?.let { !it.valid() } ?: JSONObject.NULL)
            runCatching { holder?.close() }
            socket?.let { runCatching { Os.close(it) } }
        }
        return result
    }
}
