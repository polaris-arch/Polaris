// 移植自 sing-box-for-android，文件：app/src/main/java/io/nekohasekai/sfa/bg/PlatformInterfaceWrapper.kt
// Copyright (C) 2022 by nekohasekai <contact-sagernet@sekai.icu>
// 该项目按 GNU General Public License v3（或更新版本）分发，本文件因此继承 GPLv3。
// 与上游的差异：裁剪到本仓 libbox（sing-box v1.15.0-alpha.2 tag）的 33 个方法接口面。
// 本仓不做 root 形态，需要 root 的能力一律显式关闭（见各方法注释）。
// 接口面随内核版本走：`src-tauri/core-manifest.json` 的 bundledCoreVersion 一变，
// `scripts/build-libbox.sh` 重建出来的 aar 就可能多出抽象方法，Kotlin 会在
// `compileArm64DebugKotlin` 上点名缺哪一个 —— 真值在
// `experimental/libbox/platform.go` 的 `PlatformInterface`，照它补。

package com.polaris2.app.vpn

import android.net.NetworkCapabilities
import android.os.Build
import android.os.Process
import android.provider.Settings
import android.system.OsConstants
import android.util.Log
import androidx.annotation.RequiresApi
import io.nekohasekai.libbox.AutoRedirectHandler
import io.nekohasekai.libbox.AutoRedirectSession
import io.nekohasekai.libbox.BridgeOptions
import io.nekohasekai.libbox.BridgeSession
import io.nekohasekai.libbox.ConnectionOwner
import io.nekohasekai.libbox.InterfaceUpdateListener
import io.nekohasekai.libbox.Libbox
import io.nekohasekai.libbox.LocalDNSTransport
import io.nekohasekai.libbox.NeighborUpdateListener
import io.nekohasekai.libbox.NetworkInterfaceIterator
import io.nekohasekai.libbox.PlatformInterface
import io.nekohasekai.libbox.PlatformUser
import io.nekohasekai.libbox.ShellSession
import io.nekohasekai.libbox.StringIterator
import io.nekohasekai.libbox.TunOptions
import io.nekohasekai.libbox.WIFIState
import java.net.Inet6Address
import java.net.InetSocketAddress
import java.net.InterfaceAddress
import java.net.NetworkInterface
import io.nekohasekai.libbox.NetworkInterface as LibboxNetworkInterface

/**
 * libbox `PlatformInterface` 的公共实现。
 *
 * 为什么做成 interface 而不是 class：载体是 Service 的子类（`VpnService` / 将来可能的纯代理
 * `Service`），Kotlin 单继承已经被 Service 占掉，只有接口默认实现能让两种载体共用这一份。
 * [选 A：interface + 默认实现 —— 与上游同构，载体只需覆盖它真正能做的那几个（openTun / protect / 通知）]
 * [不选 B：组合成一个持有 Context 的 class —— 每个载体都要写一遍 27 个方法的转发，且 openTun 这类
 *  必须回到载体自身（`VpnService.Builder` 是实例方法）的调用会绕一圈回来]
 */
interface PlatformInterfaceWrapper : PlatformInterface {
    // ── 出站 socket 的保护 ───────────────────────────────────────────────────────
    // 返 true = 由平台侧决定内核出站 socket 绑哪张网卡。Go 侧据此把
    // `NetworkManager.AutoDetectInterfaceFunc/ProtectFunc` 换成回调本接口
    // （sing-box v1.14.0 `route/network.go:369-376,395-403`）。返 false 会退回 Go 自己的
    // BindToInterface。Android 的物理出口还需要 netId/protect，不能只凭 Linux socket 绑定推断可用。
    override fun usePlatformAutoDetectInterfaceControl(): Boolean = true

    // 这里**必须**留空：非 VPN 载体没有 protect() 可用。VPN 载体覆盖成 `protect(fd)`。
    // 覆盖漏了的症状是「隧道建起来、握手也过、但一个字节都不通」—— 内核自己的出站 socket
    // 被自己的 tun 默认路由吃掉，形成回环，且没有任何一侧报错。
    override fun autoDetectInterfaceControl(fd: Int) {
    }

    override fun bindInterfaceControl(fd: Int, interfaceName: String) {
        InterfaceBoundSocket.bind(fd, interfaceName)
    }

    override fun openTun(options: TunOptions): Int = error("android: 该载体不持有 VpnService，无法开 tun")

    // ── 连接归因（「连接」屏按应用分组的数据来源）─────────────────────────────────
    // Android 10 (Q) 起 /proc/net 对所有应用不可读（与 targetSdk 无关），Go 侧的 procfs
    // 搜索在 Q+ 恒定失败；Q 以下 procfs 仍可用且比跨 JNI 回调便宜。
    // 注意取值时机：libbox 在 `CommandServer` 构造时**一次性**读走本方法的返回值并缓存
    // （`experimental/libbox/command_server.go:60`），运行期改不了。
    override fun useProcFS(): Boolean = Build.VERSION.SDK_INT < Build.VERSION_CODES.Q

    @RequiresApi(Build.VERSION_CODES.Q)
    override fun findConnectionOwner(
        ipProtocol: Int,
        sourceAddress: String,
        sourcePort: Int,
        destinationAddress: String,
        destinationPort: Int,
    ): ConnectionOwner {
        // 仅在 useProcFS() 为 false 时才会被调到（Go 侧 `experimental/libbox/service.go:193-227` 二选一）。
        val uid = PolarisApplication.connectivity.getConnectionOwnerUid(
            ipProtocol,
            InetSocketAddress(sourceAddress, sourcePort),
            InetSocketAddress(destinationAddress, destinationPort),
        )
        if (uid == Process.INVALID_UID) error("android: 未找到连接归属 uid")
        val packages = PolarisApplication.application.packageManager.getPackagesForUid(uid)
        return ConnectionOwner().apply {
            userId = uid
            userName = packages?.firstOrNull() ?: ""
            setAndroidPackageNames(StringArray(packages?.toList().orEmpty().iterator()))
        }
    }

    // ── 默认网络 ────────────────────────────────────────────────────────────────
    override fun startDefaultInterfaceMonitor(listener: InterfaceUpdateListener) {
        error("android: 默认网络 monitor 必须绑定原 attempt session")
    }

    override fun closeDefaultInterfaceMonitor(listener: InterfaceUpdateListener) {
        error("android: 默认网络 monitor 必须绑定原 attempt session")
    }

    @Suppress("DEPRECATION")
    override fun getInterfaces(): NetworkInterfaceIterator {
        // 两个来源必须对齐后再交给内核：ConnectivityManager 知道「这张网卡是 WiFi 还是蜂窝、计不计流量、
        // DNS 和网关是什么」，java.net.NetworkInterface 知道「index / mtu / 地址前缀」。缺任一侧的条目直接丢弃。
        val networks = PolarisApplication.connectivity.allNetworks
        val systemInterfaces = NetworkInterface.getNetworkInterfaces().toList()
        val interfaces = mutableListOf<LibboxNetworkInterface>()
        for (network in networks) {
            val linkProperties = PolarisApplication.connectivity.getLinkProperties(network) ?: continue
            val capabilities = PolarisApplication.connectivity.getNetworkCapabilities(network) ?: continue
            val systemInterface = systemInterfaces.find { it.name == linkProperties.interfaceName } ?: continue
            val boxInterface = LibboxNetworkInterface()
            boxInterface.name = linkProperties.interfaceName
            boxInterface.index = systemInterface.index
            boxInterface.dnsServer = StringArray(linkProperties.dnsServers.mapNotNull { it.hostAddress }.iterator())
            // sing-box 1.15.0-alpha.8 新增（`experimental/libbox/platform.go` 的 NetworkInterface.DNSSearchDomain）。
            // Go 侧按**一个元素一个域**消费：`service.go` 把迭代器摊成 `[]string`，`dns_search_domain`
            // 规则逐元素 `mDNS.CanonicalName` 后比对 ⇒ 大小写与末尾点不用管，但绝不能把整串塞成一个元素。
            // `LinkProperties.getDomains()` 给的是**一整串**（AOSP 文档写空格分隔，旧版本/部分 OEM 见过逗号），
            // 故两种分隔符都切，空段丢掉；null（该网络没下发搜索域）⇒ 空迭代器。
            boxInterface.dnsSearchDomain = StringArray(
                linkProperties.domains.orEmpty()
                    .split(',', ' ', '\t')
                    .filter { it.isNotBlank() }
                    .iterator(),
            )
            boxInterface.gateway = StringArray(
                linkProperties.routes
                    .filter { it.destination.prefixLength == 0 }
                    .mapNotNull { it.gateway }
                    .filterNot { it.isAnyLocalAddress }
                    .mapNotNull { it.hostAddress }
                    .iterator(),
            )
            boxInterface.type = when {
                capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> Libbox.InterfaceTypeWIFI
                capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> Libbox.InterfaceTypeCellular
                capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> Libbox.InterfaceTypeEthernet
                else -> Libbox.InterfaceTypeOther
            }
            // 某些 OEM 上取 mtu 会抛 SocketException（网卡刚消失），不该让整份枚举失败。
            runCatching { boxInterface.mtu = systemInterface.mtu }
                .onFailure { Log.w(TAG, "取 ${boxInterface.name} 的 mtu 失败", it) }
            boxInterface.addresses =
                StringArray(systemInterface.interfaceAddresses.map { it.toPrefix() }.iterator())
            // Go 侧只认 IFF_* 位（`experimental/libbox/service.go` → `linkFlags`），而 Java 不暴露真实
            // 接口 flags，只能从能力位反推。NET_CAPABILITY_INTERNET 是「这张网卡可用」的最近似判据。
            var flags = 0
            if (capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)) {
                flags = OsConstants.IFF_UP or OsConstants.IFF_RUNNING
            }
            if (systemInterface.isLoopback) flags = flags or OsConstants.IFF_LOOPBACK
            if (systemInterface.isPointToPoint) flags = flags or OsConstants.IFF_POINTOPOINT
            if (systemInterface.supportsMulticast()) flags = flags or OsConstants.IFF_MULTICAST
            boxInterface.flags = flags
            boxInterface.metered = !capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
            interfaces.add(boxInterface)
        }
        return InterfaceArray(interfaces.iterator())
    }

    // ── Apple 专有语义：在 Android 上恒 false ────────────────────────────────────
    // underNetworkExtension 说的是「本进程跑在 Apple 的 NEPacketTunnelProvider 里」。
    override fun underNetworkExtension(): Boolean = false

    // includeAllNetworks 只在 underNetworkExtension() 为 true 时才被 Go 侧读取
    // （`protocol/tun/inbound.go:448-451`），因而在 Android 上返什么都不会被消费。
    // 保留为 false 是为了「万一将来 underNetworkExtension 改了」时不会静默切到 gVisor-only 路径：
    // sing-tun 在 IncludeAllNetworks=true 时拒绝 system/mixed 栈（`stack.go:53-61` → ErrIncludeAllNetworks）。
    override fun includeAllNetworks(): Boolean = false

    // Android 没有进程级 DNS 缓存可清（解析走 netd / DnsResolver）。
    override fun clearDNSCache() {
    }

    @Suppress("DEPRECATION")
    override fun readWIFIState(): WIFIState? {
        val wifiInfo = PolarisApplication.wifi.connectionInfo ?: return null
        var ssid = wifiInfo.ssid ?: return WIFIState("", "")
        // 无定位权限时系统返回字面量 "<unknown ssid>"，那不是 SSID，别当成一个网络名传下去。
        if (ssid == "<unknown ssid>") return WIFIState("", "")
        if (ssid.startsWith("\"") && ssid.endsWith("\"")) ssid = ssid.substring(1, ssid.length - 1)
        return WIFIState(ssid, wifiInfo.bssid ?: "")
    }

    override fun localDNSTransport(): LocalDNSTransport? = LocalResolver

    // ── 邻居表 / 特权外壳 / bridge：本仓不做 root 形态，一律显式关闭 ───────────────
    // 上游这三块都靠 libsu 起一个 root 服务（`RootServer.kt`）。没有 root 时它们的正确形态是
    // 「不声明能力」，而不是「声明了再在调用时抛」——后者会让内核以为该能力可用并走到一半才失败。
    override fun startNeighborMonitor(listener: NeighborUpdateListener?) {
    }

    override fun closeNeighborMonitor(listener: NeighborUpdateListener?) {
    }

    override fun usePlatformShell(): Boolean = false

    override fun checkPlatformShell() {
        error("android: 未实现特权外壳")
    }

    override fun openShellSession(
        user: PlatformUser?,
        command: String?,
        environ: StringIterator?,
        term: String?,
        rows: Int,
        cols: Int,
    ): ShellSession = error("android: 未实现特权外壳")

    override fun lookupUser(username: String?): PlatformUser = error("android: 未实现用户查询")

    override fun lookupSFTPServer(): String = error("android: 未实现 SFTP 服务")

    override fun readSystemSSHHostKey(): String = error("android: 未实现系统 SSH host key")

    override fun usePlatformBridge(): Boolean = false

    override fun createBridge(options: BridgeOptions?): BridgeSession = error("android: 未实现 bridge")

    // auto-redirect：内核在 tun 之外再起一个 tproxy/redirect 入口，靠 nftables/iptables 把流量
    // 劫进来（`experimental/libbox/redirect_linux.go`）。改防火墙表要 root，上游安卓端也是先问
    // `RootClient.checkRootAvailable()` 再决定。本仓不做 root 形态 ⇒ 与 shell / bridge 同处理：
    // 不声明能力，声明后才抛会让内核以为可用、走到一半才失败。
    override fun usePlatformAutoRedirect(): Boolean = false

    override fun createAutoRedirect(
        options: ByteArray?,
        handler: AutoRedirectHandler?,
    ): AutoRedirectSession = error("android: 未实现 auto-redirect")

    // Tailscale 端点用它作为本机在 tailnet 里的名字（`protocol/tailscale/endpoint.go:144`）。
    // 与 root 无关，给一个稳定可读的值即可。
    override fun tailscaleHostname(): String =
        Settings.Global.getString(PolarisApplication.application.contentResolver, Settings.Global.DEVICE_NAME)
            ?.takeIf { it.isNotBlank() }
            ?: "${Build.MANUFACTURER} ${Build.MODEL}"

    // tun 名字由 libbox 从 fd 反查后回传，本地暂无消费点。
    override fun registerMyInterface(name: String?) {
    }

    companion object {
        private const val TAG = "PolarisPlatform"
    }

    // ── gomobile 迭代器适配 ─────────────────────────────────────────────────────
    class StringArray(private val iterator: Iterator<String>) : StringIterator {
        // Go 侧读迭代器只用 HasNext/Next，Len 未被消费（上游同）。
        override fun len(): Int = 0

        override fun hasNext(): Boolean = iterator.hasNext()

        override fun next(): String = iterator.next()
    }

    private class InterfaceArray(private val iterator: Iterator<LibboxNetworkInterface>) : NetworkInterfaceIterator {
        override fun hasNext(): Boolean = iterator.hasNext()

        override fun next(): LibboxNetworkInterface = iterator.next()
    }
}

// IPv6 地址要走 getByAddress 重新格式化：InterfaceAddress.getAddress().getHostAddress() 会带上
// scope id（如 "fe80::1%wlan0"），而 Go 侧用 netip.MustParsePrefix 解析，带 scope 会直接 panic。
private fun InterfaceAddress.toPrefix(): String = if (address is Inet6Address) {
    "${Inet6Address.getByAddress(address.address).hostAddress}/$networkPrefixLength"
} else {
    "${address.hostAddress}/$networkPrefixLength"
}
