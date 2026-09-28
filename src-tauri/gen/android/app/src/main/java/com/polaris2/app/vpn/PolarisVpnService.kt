// 移植自 sing-box-for-android，文件：app/src/main/java/io/nekohasekai/sfa/bg/VPNService.kt
// Copyright (C) 2022 by nekohasekai <contact-sagernet@sekai.icu>
// 该项目按 GNU General Public License v3（或更新版本）分发，本文件因此继承 GPLv3。
// 与上游的差异：去掉 Settings 驱动的 allowBypass / systemProxy 开关（本仓尚无对应设置面），
// 其余（地址、路由、排除路由、按应用分流、TIRAMISU 前后的两套路由 API）逐条对齐。

package com.polaris2.app.vpn

import android.content.Intent
import android.content.pm.PackageManager.NameNotFoundException
import android.net.IpPrefix
import android.net.VpnService
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.annotation.RequiresApi
import io.nekohasekai.libbox.Libbox
import io.nekohasekai.libbox.Notification
import io.nekohasekai.libbox.RoutePrefix
import io.nekohasekai.libbox.TunOptions
import java.net.InetAddress

/**
 * tun 的载体。
 *
 * 整件事的题眼在 `openTun` 的返回值：那是一个**进程内**才有意义的 int（fd）。内核拿到它之后
 * 直接 dup 并读写，没有任何跨进程传递 —— 这正是内核必须与 VpnService 同进程（libbox 方案）的原因。
 */
class PolarisVpnService :
    VpnService(),
    PlatformInterfaceWrapper {
    private val boxService = BoxService(this, this)

    override fun onCreate() {
        super.onCreate()
        socketProtector = this
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = boxService.onStartCommand()

    override fun onBind(intent: Intent): IBinder? {
        // super.onBind 只在 action 是 SERVICE_INTERFACE 时返回系统 binder（系统用它探测 VPN 组件）；
        // 吞掉它会让系统认为这不是一个合法的 VpnService。
        return super.onBind(intent) ?: boxService.onBind()
    }

    override fun onDestroy() {
        boxService.onDestroy()
        if (socketProtector === this) socketProtector = null
        super.onDestroy()
    }

    override fun onRevoke() {
        boxService.onRevoke()
        super.onRevoke()
    }

    // 承重：内核自己的出站 socket 必须走 protect()，否则它会命中我们刚装上的默认路由，
    // 数据包绕回 tun 形成回环。症状是「隧道建起来了、状态显示已连接、但一个字节都不通」，
    // 且两侧都不报错 —— 只有抓包才看得出来。
    override fun autoDetectInterfaceControl(fd: Int) {
        protect(fd)
    }


    override fun openTun(options: TunOptions): Int {
        if (prepare(this) != null) error("android: 缺少 VPN 授权")

        val builder = Builder()
            .setSession(SESSION_NAME)
            .setMtu(options.mtu)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            // 隧道本身不该被当成计费网络：底层是 WiFi 还是蜂窝由 underlying network 决定，
            // 在这里报 metered=true 会让系统里所有「仅 WiFi」的策略在隧道下全部失效。
            builder.setMetered(false)
        }

        // 不调 allowBypass()：那会允许任意应用用 Network.bindSocket 绕开隧道。它在上游是一个
        // 用户可见开关；本仓尚无该设置面，缺省取"不可绕过"这一侧 —— 默认值应当偏向不漏流量。

        options.inet4Address.forEach { builder.addAddress(it.address(), it.prefix()) }
        options.inet6Address.forEach { builder.addAddress(it.address(), it.prefix()) }

        if (options.autoRoute) {
            if (options.dnsMode.value != Libbox.DNSModeDisabled) {
                val dnsServers = options.dnsServerAddress
                while (dnsServers.hasNext()) builder.addDnsServer(dnsServers.next())
            }
            applyRoutes(builder, options)
            applyPackageFilter(builder, options)
        }

        if (options.isHTTPProxyEnabled && Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            builder.setHttpProxy(
                android.net.ProxyInfo.buildDirectProxy(
                    options.httpProxyServer,
                    options.httpProxyServerPort,
                    options.httpProxyBypassDomain.toList(),
                ),
            )
        }

        val pfd = builder.establish() ?: error("android: VPN 未授权或已被撤销")
        boxService.fileDescriptor = pfd
        return pfd.fd
    }

    private fun applyRoutes(builder: Builder, options: TunOptions) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            applyRoutesTiramisu(builder, options)
            return
        }
        // TIRAMISU 之前没有 excludeRoute：内核把「全量路由减去排除项」预先算成一组range 交过来
        // （Go 侧 `tun.Options.BuildAutoRouteRanges`），这里只能整组加进去。
        options.inet4RouteRange.forEach { builder.addRoute(it.address(), it.prefix()) }
        options.inet6RouteRange.forEach { builder.addRoute(it.address(), it.prefix()) }
    }

    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private fun applyRoutesTiramisu(builder: Builder, options: TunOptions) {
        // 一条 route 都不给 = 隧道装不上任何路由 = 什么都不走它。所以「内核没指定具体路由」
        // 必须回落成默认路由，而不是留空。这一分支上游同款。
        val inet4Routes = options.inet4RouteAddress.toList()
        if (inet4Routes.isNotEmpty()) {
            inet4Routes.forEach { builder.addRoute(it.toIpPrefix()) }
        } else if (options.inet4Address.hasNext()) {
            builder.addRoute("0.0.0.0", 0)
        }

        val inet6Routes = options.inet6RouteAddress.toList()
        if (inet6Routes.isNotEmpty()) {
            inet6Routes.forEach { builder.addRoute(it.toIpPrefix()) }
        } else if (options.inet6Address.hasNext()) {
            builder.addRoute("::", 0)
        }

        options.inet4RouteExcludeAddress.forEach { excludeRouteTolerantly(builder, it) }
        options.inet6RouteExcludeAddress.forEach { excludeRouteTolerantly(builder, it) }
    }

    /**
     * 逐条加排除路由，被系统拒收的那一条跳过并**点名**记日志。
     *
     * 🔴 成因（模拟器实测，非推断）：`VpnService.Builder.excludeRoute` 对 **loopback 前缀**直接抛
     * `IllegalArgumentException("Bad address")`（AOSP `Builder.check()` 的第一条判定），而
     * config-engine 生成的 `tun.route_exclude_address` 里恒含 `127.0.0.0/8` 与 `::1/128`
     * （桌面侧那是正确且必要的）。整条 `forEach` 不容错时，第一条 loopback 就让 `openTun` 抛出去，
     * 内核报 `start inbound/tun[tun-in]: configure tun interface: Bad address`，**隧道一次都建不起来**。
     *
     * **这只是承载侧的容错，不是根因的收口。** 生成侧的收口 2026-09-06 已落地，分两处：
     *  · 平台基线：`builder/inbounds.rs` 的 android 臂基线为空集（回环排除在 Android 上本就无意义，
     *    系统从不把回环交给 VPN）；开着「绕过局域网」时加上清单网段子集（2026-09-25），其中的
     *    `127.0.0.0/8` 同样在生成侧剔掉（`mobile_bypass_lan_exclude`）；
     *  · 用户声明段（`tunConfig.inboundExcludeCidrs`，移动端 `settings/TunPage.tsx` 那一块）：
     *    `builder/inbounds.rs` 的 `drop_mobile_loopback_excludes` 按
     *    `IpAddr::is_loopback()`（= AOSP `Builder.check()` 的同一条判据）逐条剔掉并 warn。
     *    门在 `crates/config-engine/tests/golden_tun_inbound_exclude.rs` 的
     *    `android_keeps_user_segments_but_drops_loopback_prefixes`。
     *
     * 🔴 **即便如此这一层也不许删**：系统还会拒收生成侧枚举不了的别的形态（同一个 Builder 对多播 /
     * 保留段也挑），而代价上限是「一条被拒 = 整条隧道建不起来」。两道都要。
     * 这里的形态与 [`applyPackageFilter`] 逐条容错**刻意同构**：同一个理由
     * （一条失败不该让整个隧道建不起来），同一种处置（跳过 + 点名警告，绝不静默）。
     */
    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private fun excludeRouteTolerantly(builder: Builder, prefix: RoutePrefix) {
        runCatching { builder.excludeRoute(prefix.toIpPrefix()) }
            .onFailure {
                Log.w(TAG, "排除路由 ${prefix.address()}/${prefix.prefix()} 被系统拒收，已跳过：${it.message}")
            }
    }

    private fun applyPackageFilter(builder: Builder, options: TunOptions) {
        // 按应用分流：Android 上取代桌面的 process_name / process_path 规则。
        // 包名可能已经卸载（配置里留着旧条目），逐条容错 —— 一条失败不该让整个隧道建不起来。
        val includePackage = options.includePackage
        while (includePackage.hasNext()) {
            val name = includePackage.next()
            runCatching { builder.addAllowedApplication(name) }
                .onFailure { if (it is NameNotFoundException) Log.w(TAG, "包不存在，跳过 include: $name") else throw it }
        }
        val excludePackage = options.excludePackage
        while (excludePackage.hasNext()) {
            val name = excludePackage.next()
            runCatching { builder.addDisallowedApplication(name) }
                .onFailure { if (it is NameNotFoundException) Log.w(TAG, "包不存在，跳过 exclude: $name") else throw it }
        }
    }

    override fun sendNotification(notification: Notification) = boxService.sendNotification(notification)

    override fun cancelNotification(identifier: String, typeID: Int) =
        boxService.cancelNotification(identifier, typeID)

    companion object {
        @Volatile private var socketProtector: PolarisVpnService? = null

        internal fun hasSocketProtector(): Boolean = socketProtector != null

        /** App-owned network sockets must not route back into our active VPN. No VPN is started here. */
        internal fun protectTransientSocket(fd: Int) {
            socketProtector?.let { check(it.protect(fd)) { "android: 无法保护网络 socket" } }
        }
        private const val TAG = "PolarisVpnService"
        private const val SESSION_NAME = "Polaris"
    }
}

// gomobile 的迭代器不是 Iterable，每处都写 while(hasNext) 会把上面几段的意图淹掉。
private inline fun io.nekohasekai.libbox.RoutePrefixIterator.forEach(action: (RoutePrefix) -> Unit) {
    while (hasNext()) action(next())
}

private fun io.nekohasekai.libbox.RoutePrefixIterator.toList(): List<RoutePrefix> {
    val list = mutableListOf<RoutePrefix>()
    while (hasNext()) list.add(next())
    return list
}

private fun io.nekohasekai.libbox.StringIterator.toList(): List<String> {
    val list = mutableListOf<String>()
    while (hasNext()) list.add(next())
    return list
}

@RequiresApi(Build.VERSION_CODES.TIRAMISU)
private fun RoutePrefix.toIpPrefix(): IpPrefix = IpPrefix(InetAddress.getByName(address()), prefix())
