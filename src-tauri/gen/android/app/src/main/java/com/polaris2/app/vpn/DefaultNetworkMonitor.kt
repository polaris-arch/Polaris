// 移植自 sing-box-for-android：
//   app/src/main/java/io/nekohasekai/sfa/bg/DefaultNetworkMonitor.kt
//   app/src/main/java/io/nekohasekai/sfa/bg/DefaultNetworkListener.kt（register() 的版本分支与其注释）
// Copyright (C) 2022 by nekohasekai <contact-sagernet@sekai.icu>
// DefaultNetworkListener.kt 另带上游署名：
//   Copyright (C) 2019 by Max Lv <max.c.lv@gmail.com>
//   Copyright (C) 2019 by Mygod Studio <contact-shadowsocks-android@mygod.be>
// 两者均按 GNU General Public License v3（或更新版本）分发，本文件因此继承 GPLv3。

package com.polaris2.app.vpn

import android.annotation.SuppressLint
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import io.nekohasekai.libbox.InterfaceUpdateListener
import io.nekohasekai.libbox.Libbox
import java.net.NetworkInterface

/**
 * 「当前默认网络是哪一张」的唯一来源。
 *
 * 内核用它做两件事：出站 socket 绑到哪张网卡（经 `autoDetectInterfaceControl`）、DNS 查询走哪张网卡
 * （`LocalResolver`）。切网（WiFi ↔ 蜂窝）时它必须及时更新，否则连接会挂在一张已经消失的网卡上。
 *
 * [选 A：直接 NetworkCallback + volatile 字段 —— 只有一个消费者（libbox），只需要「当前网络」与
 *  「变了通知一次」两件事]
 * [不选 B：整套移植上游的 `DefaultNetworkListener` actor —— 那是 shadowsocks-android 为多监听者 +
 *  可挂起获取设计的，会引入 kotlinx-coroutines 的 actor（ObsoleteCoroutinesApi）与 GlobalScope，
 *  多一个并发模型却只服务一个消费者]
 *
 * 从上游**照抄不动**的是 `register()` 里的版本分支：见其注释。
 */
object DefaultNetworkMonitor {
    private const val TAG = "PolarisNetMonitor"

    private val mainHandler = Handler(Looper.getMainLooper())
    private val coordinator = NetworkMonitorCoordinator<Network, InterfaceUpdateListener>(
        registration = { events ->
            // This SDK callback is born with the exact attempt registration.
            val callback = object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) = events.available(network)
                override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) =
                    events.capabilitiesChanged(network)
                override fun onLost(network: Network) = events.lost(network)
            }
            object : NetworkMonitorRegistration {
                override fun register() = register(callback)
                override fun unregister() {
                    try { PolarisApplication.connectivity.unregisterNetworkCallback(callback) }
                    catch (error: Throwable) { Log.w(TAG, "注销网络回调失败", error); throw error }
                }
            }
        },
        initialNetwork = { PolarisApplication.connectivity.activeNetwork },
        lookup = { network -> lookup(network) },
        identity = Libbox::interfaceUpdateListenerIdentity,
        deliver = { listener, update ->
            listener.updateDefaultInterface(update.name, update.index, update.expensive, update.constrained)
        },
    )

    val defaultNetwork: Network? get() = coordinator.defaultNetwork
    internal fun createSession(): NetworkMonitorSession<Network, InterfaceUpdateListener> = coordinator.createSession()

    private fun lookup(network: Network): NetworkMonitorUpdate? {
        // Link/index lookup and its original retry behavior stay outside the gate.
        // A slow result still needs a revision/identity permit before JNI.
        for (attempt in 0 until 10) {
            val properties = PolarisApplication.connectivity.getLinkProperties(network)
            val name = properties?.interfaceName
            val index = name?.let { runCatching { NetworkInterface.getByName(it)?.index }.getOrNull() }
            if (name != null && index != null) return NetworkMonitorUpdate(name, index)
            Thread.sleep(100)
        }
        Log.w(TAG, "10 次重试后仍拿不到默认网卡 index")
        return null
    }

    /**
     * 以下版本分支从上游 `DefaultNetworkListener.register()` 照抄，连同它的成因注释：
     *
     * > Unfortunately registerDefaultNetworkCallback is going to return VPN interface since Android P DP1:
     * > https://android.googlesource.com/platform/frameworks/base/+/dda156ab0c5d66ad82bdcf76cda07cbc0a9c8a2e
     * > This makes doing a requestNetwork with REQUEST necessary so that we don't get ALL possible networks
     * > that satisfies default network capabilities but only THE default network.
     *
     * 对本仓的意义是承重的：用错 API 会让「默认网络」在 VPN 起来后指向我们自己的 tun，
     * 于是内核把自己的出站 socket 绑回自己的隧道，形成回环。
     */
    @SuppressLint("MissingPermission") // CHANGE_NETWORK_STATE 已在 Manifest 声明，lint 认不出跨方法的分支
    private fun register(callback: ConnectivityManager.NetworkCallback) {
        val request = NetworkRequest.Builder().apply {
            addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_RESTRICTED)
        }.build()
        val connectivity = PolarisApplication.connectivity
        when {
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
                connectivity.registerBestMatchingNetworkCallback(request, callback, mainHandler)

            // 28..30：要 REQUEST 语义而不是 LISTEN，故用 requestNetwork（需 CHANGE_NETWORK_STATE 权限）。
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.P ->
                connectivity.requestNetwork(request, callback, mainHandler)

            // 26..27：带 Handler 的重载在 26 才有；本仓 minSdk 24，24..25 只能用无 Handler 的那个。
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.O ->
                connectivity.registerDefaultNetworkCallback(callback, mainHandler)

            else -> connectivity.registerDefaultNetworkCallback(callback)
        }
    }
}
