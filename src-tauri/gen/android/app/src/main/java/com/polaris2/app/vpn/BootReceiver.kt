// 本仓自有文件（非移植）。
package com.polaris2.app.vpn

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.VpnService
import android.util.Log
import androidx.core.content.ContextCompat

/**
 * 「开机自动连接」（设置 → 通用，`autoStart` 字段的 Android 腿）的执行者。
 *
 * # 与系统「始终开启的 VPN」的分工
 *
 * always-on 由系统持有：系统在开机、隧道掉线时都会主动拉起 `PolarisVpnService`，还能拦截未走 VPN
 * 的流量 —— 那条路不经过本类，配置来源同样是 [SystemStart.load]。本类是**应用自己的**开关：
 * 只在开机那一刻起一次，不负责掉线重连、不拦截流量。两者可同时开：先到的那个起核，后到的那次
 * 起服务意图被 `BoxService.onStartCommand` 判成「已在起停中」只留日志。
 *
 * # 四个条件缺一不起（每条都留一行日志 —— 开机时没有界面，logcat 是唯一证据）
 *
 *  1. 开关为开（[SystemStart.bootAutoConnect]）；
 *  2. VPN 已授权：`VpnService.prepare` 返回 `null`。未授权时起服务只会在 `openTun` 失败，且授权弹窗
 *     需要一个 Activity，开机时没有；
 *  3. 用户最后一次状态不是「主动断开」且有可用的落盘配置：[SystemStart.load] 能取到
 *     （准入摘要在 + 盘上文件与之同一份）；
 *  4. 前台服务起得来（Android 12+ 后台起前台服务受限，`BOOT_COMPLETED` 在豁免名单里；
 *     Android 15 起它不许拉起 dataSync/camera/mediaPlayback/phoneCall/mediaProjection/microphone
 *     六类前台服务，本服务是 `systemExempted`，不在其列 —— 出处 developer.android.com
 *     「Behavior changes: Android 15」。`systemExempted` 自身的准入是「VPN apps（在 设置 → VPN 里
 *     配置过的）」，条件 2 的「已授权」正是它的前提；两条都只核了文档，**未经真机**）。
 *     抛了就记日志，不重试。
 *
 * 只收 `BOOT_COMPLETED`（凭据加密存储已解锁之后才发）：`LOCKED_BOOT_COMPLETED` 时 Rust 落盘的配置
 * 在 CE 存储里读不到，收它只会得到一次必然失败的起核。
 *
 * `exported=false`：`BOOT_COMPLETED` 由 system uid 投递，不受 exported 限制；关掉导出让别的应用
 * 没法伪造一次开机广播来替用户连上隧道。
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
        if (!SystemStart.bootAutoConnect(context)) {
            Log.i(TAG, "开机自动连接未开启，不起核")
            return
        }
        if (VpnService.prepare(context) != null) {
            Log.w(TAG, "开机自动连接：未授予 VPN 权限，不起核（需在应用内连接一次完成授权）")
            return
        }
        // 只做准入与可用性判定；真正起核时服务会再读一次（两次之间文件不会被写：Rust 此刻没在跑）。
        val refused = runCatching { SystemStart.load(context) }.exceptionOrNull()
        if (refused != null) {
            Log.i(TAG, "开机自动连接：${refused.message}")
            return
        }
        try {
            ContextCompat.startForegroundService(context, Intent(context, PolarisVpnService::class.java))
            Log.i(TAG, "开机自动连接：已请求起核")
        } catch (e: Throwable) {
            Log.e(TAG, "开机自动连接：起前台服务失败", e)
        }
    }

    private companion object {
        const val TAG = "PolarisBootReceiver"
    }
}
