// 移植自 sing-box-for-android，文件：app/src/main/java/io/nekohasekai/sfa/bg/BoxService.kt
// Copyright (C) 2022 by nekohasekai <contact-sagernet@sekai.icu>
// 该项目按 GNU General Public License v3（或更新版本）分发，本文件因此继承 GPLv3。
// 与上游的差异：
//   - 不引 kotlinx-coroutines（上游用 GlobalScope + Dispatchers.IO），改用一条命名线程；
//   - 去掉 profile 数据库 / per-app 代理 / OOM 与 power 报告 / 系统代理开关 —— 那些各自依赖上游
//     的 Settings 与 UI，本批射程只到「内核能起、tun 能建」；
//   - 配置来源是 Rust 侧 config-engine 的产物，经 PolarisVpnPlugin 的 start 命令进 VpnBridge；
//     系统发起的起核（无桥调用）改读 Rust 落盘的同一份文件（SystemStart）。

package com.polaris2.app.vpn

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import com.polaris2.app.MainActivity
import com.polaris2.app.R
import io.nekohasekai.libbox.CommandServer
import io.nekohasekai.libbox.CommandServerHandler
import io.nekohasekai.libbox.Notification
import io.nekohasekai.libbox.OverrideOptions
import io.nekohasekai.libbox.PlatformInterface
import io.nekohasekai.libbox.SystemProxyStatus

/**
 * 内核生命周期：把 Android 的 Service 回调翻译成 libbox 的 CommandServer 生命周期。
 *
 * 为什么不做成 Service 的基类：载体必须是 `VpnService` 的子类（tun 只能由它建），而将来可能出现
 * 的纯代理载体是普通 `Service`。把生命周期做成被持有的对象，两种载体只需各自转发几个回调。
 * 上游同构（`VPNService` / `ProxyService` 都 `private val service = BoxService(this, this)`）。
 */
class BoxService(
    private val service: Service,
    private val platformInterface: PlatformInterface,
) : CommandServerHandler {
    /** VPN 载体在 openTun 成功后写进来。停服务时必须关它，否则系统认为隧道还在。 */
    var fileDescriptor: ParcelFileDescriptor? = null

    @Volatile
    var state: ServiceState = ServiceState.Stopped
        private set

    private val mainHandler = Handler(Looper.getMainLooper())
    private val notification = ServiceNotification(service)
    private var commandServer: CommandServer? = null
    private var receiverRegistered = false

    private val stopReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action != ServiceAction.SERVICE_STOP) return
            // 这条广播只有两个发送方：通知栏「断开」与 `PolarisVpnPlugin.stop`（经 requestStop）——
            // 两者都是「用户要它断开」。撤销准入，否则系统下次拉起服务会违背用户意图把它连回去。
            SystemStart.forget(context, "用户断开")
            stopService()
        }
    }

    fun onStartCommand(): Int {
        if (state != ServiceState.Stopped) {
            // 桥若欠着一个起核回执而这一路走不下去了 —— 必须当场结账。静默返回等于让 Rust
            // 侧一直等到桥超时，而「超时」与「服务拒绝重复起核」在那一侧完全不可区分。
            // 没人欠回执（系统在隧道已在跑时重申 always-on）⇒ 只记日志，**不动**桥的 running 位。
            VpnBridge.rejectPendingStart("android: 服务处于 $state 状态，拒绝重复起核")
            return Service.START_NOT_STICKY
        }
        // 没有桥交来的配置 ⇒ 这是系统发起的起核（always-on / 开机接收器 / 进程被回收后重拉）。
        // 先在桥上记账（挡住并发的 `start`），配置在工作线程里由 SystemStart.load 读。
        if (VpnBridge.currentConfig() == null && !VpnBridge.beginSystemStart()) {
            Log.w(TAG, "系统发起的起核撞上桥正忙，仍按系统起核处理")
        }
        state = ServiceState.Starting
        // 先立前台通知：startForeground 的 5 秒窗口从 onStartCommand 起算，而内核起来要秒级。
        notification.show(ServiceState.Starting, PROFILE_NAME)
        registerStopReceiver()
        Thread({ startKernel() }, "polaris-box-start").start()
        return Service.START_NOT_STICKY
    }

    fun onBind() = null

    fun onDestroy() {
        stopService()
    }

    fun onRevoke() {
        // 用户在系统设置里撤销了 VPN 授权，或另一个 VPN 应用抢走了隧道。两者都意味着「不该再自启」。
        SystemStart.forget(service, "VPN 授权被撤销")
        stopService()
    }

    private fun startKernel() {
        // 桥交来的那一份（经桥起核）；`null` = 系统发起的起核，配置读 Rust 落盘的同一份文件。
        val bridgeConfig = VpnBridge.currentConfig()
        try {
            val config = bridgeConfig ?: SystemStart.load(service)
            SystemEndpointGuard.requireSupported(config)
            PolarisApplication.ensureSetup()
            TransientLoginHost.withMainConfig(this, config, { state == ServiceState.Starting }) {
                check(state == ServiceState.Starting) { "起核已被停核接管" }
                DefaultNetworkMonitor.start()
                val server = CommandServer(this, platformInterface)
                commandServer = server
                server.start()
                // No login instance may hold this Tailscale state directory during main startup.
                server.startOrReloadService(config, OverrideOptions())
            }
            synchronized(this) {
                // stopService 持同一把锁：停核已接管时不可重新放开命令流。
                check(state == ServiceState.Starting) { "起核已被停核接管" }
                state = ServiceState.Started
                StatsBridge.activateAll()
            }
            mainHandler.post { notification.onStarted() }
            Log.i(TAG, "内核已启动")
            // 结账必须排在 state=Started 之后：Rust 侧收到 resolve 就会立刻开始就绪门探测。
            VpnBridge.finishStart(null)
            // 经桥起核成功 ⇒ 记下这一份，此后系统发起的起核才有配置可用。排在结账之后：它要读一次
            // 盘并算摘要，不该拖慢 Rust 侧的就绪门。失败只影响「系统起核可用性」，不影响本次连接。
            if (bridgeConfig != null) {
                runCatching { SystemStart.remember(service, bridgeConfig) }
                    .onFailure { Log.e(TAG, "记录起核配置失败，系统发起的起核将不可用", it) }
            }
        } catch (e: Throwable) {
            // 🔴 这条 catch 此前只 Log.e + stopService()，Rust 侧什么都收不到 —— 那正是
            // 「静默没起来」的现场。先结账（把内核原话带回去），再拆自己。
            Log.e(TAG, "内核启动失败", e)
            // 经桥起核失败：盘上那份已是这次失败的尝试，旧摘要作废。系统起核失败**不撤**准入：
            // 用户的意图没变（仍是「连着」），下一次系统重试 / 用户手动连接自然会覆盖。
            if (bridgeConfig != null) SystemStart.forget(service, "经桥起核失败")
            VpnBridge.finishStart(
                e.message ?: e.toString(),
                if (e is SystemEndpointGuard.Unsupported) SystemEndpointGuard.ERROR else PolarisVpnPlugin.ERR_STARTUP_FAILED,
            )
            stopService()
        }
    }

    // ── CommandServerHandler ────────────────────────────────────────────────────

    override fun serviceStop() {
        stopService()
    }

    override fun serviceReload() {
        val server = commandServer ?: return
        // 系统发起的核没有桥配置 ⇒ 与起核同源，读 Rust 落盘的那一份。
        val config = VpnBridge.currentConfig()
            ?: runCatching { SystemStart.load(service) }.getOrElse {
                server.setError("android: reload: ${it.message}")
                return
            }
        try {
            SystemEndpointGuard.requireSupported(config)
        } catch (error: Exception) {
            server.setError("android: reload: ${error.message}")
            return
        }
        runCatching { TransientLoginHost.withMainConfig(this, config, { state == ServiceState.Started && commandServer === server }) { server.startOrReloadService(config, OverrideOptions()) } }
            .onFailure {
                Log.e(TAG, "重载失败", it)
                server.setError("android: reload: ${it.message}")
            }
    }

    // Android 没有「系统 HTTP 代理开关」这一层：VpnService.Builder.setHttpProxy 是随隧道一起
    // 生效的，不是可独立开关的状态。故恒报不可用，而不是假装可切。
    override fun getSystemProxyStatus(): SystemProxyStatus = SystemProxyStatus().apply {
        available = false
        enabled = false
    }

    override fun setSystemProxyEnabled(isEnabled: Boolean) {
    }

    override fun triggerNativeCrash() {
        Thread {
            Thread.sleep(200)
            throw RuntimeException("debug native crash")
        }.start()
    }

    override fun writeDebugMessage(message: String?) {
        Log.d(TAG, message ?: return)
    }

    // SSH agent 转发要一个能连到宿主 agent 的 fd，Android 上没有对应物。
    override fun connectSSHAgent(): Int = -1

    // ── 内核发起的通知（证书过期、Tailscale 需要登录之类）────────────────────────
    // 与前台服务那条常驻通知分开：这些是**事件**，可点掉，且各自有自己的频道。

    fun sendNotification(notification: Notification) {
        val channel = "polaris-event-${notification.typeID}"
        val builder = NotificationCompat.Builder(service, channel)
            .setShowWhen(false)
            .setContentTitle(notification.title)
            .setContentText(notification.body)
            .setOnlyAlertOnce(true)
            .setAutoCancel(true)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setCategory(NotificationCompat.CATEGORY_EVENT)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
        if (!notification.subtitle.isNullOrBlank()) {
            builder.setSubText(notification.subtitle)
        }
        if (!notification.openURL.isNullOrBlank()) {
            builder.setContentIntent(
                PendingIntent.getActivity(
                    service,
                    0,
                    Intent(service, MainActivity::class.java)
                        .setAction(Intent.ACTION_VIEW)
                        .setData(Uri.parse(notification.openURL))
                        .setFlags(Intent.FLAG_ACTIVITY_REORDER_TO_FRONT),
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                        PendingIntent.FLAG_IMMUTABLE
                    } else {
                        0
                    },
                ),
            )
        }
        val identifier = notification.identifier
        val typeID = notification.typeID
        val typeName = notification.typeName
        mainHandler.post {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                PolarisApplication.notification.createNotificationChannel(
                    NotificationChannel(channel, typeName, NotificationManager.IMPORTANCE_HIGH),
                )
            }
            PolarisApplication.notification.notify(identifier, typeID, builder.build())
        }
    }

    fun cancelNotification(identifier: String, typeID: Int) {
        mainHandler.post { PolarisApplication.notification.cancel(identifier, typeID) }
    }

    // ── 停机 ────────────────────────────────────────────────────────────────────

    @Volatile private var closeFailed = false

    @Synchronized
    private fun stopService() {
        // 已有一条停机在飞：它自己会在末尾结账，这里再结一次会让 Rust 在 tun 还没拆完时就收到「已停」。
        if (state == ServiceState.Stopping && !closeFailed) return
        if (state == ServiceState.Stopped) {
            // 服务早就没了（被系统回收 / 起核失败时已自行停过），但桥可能仍欠着一个停核回执。
            // 结账才能让「本就没在跑」与「停不掉」在 Rust 侧区分开。
            VpnBridge.finishStop()
            return
        }
        closeFailed = false
        state = ServiceState.Stopping
        // 当场封住新 CommandClient：Rust 的 running 位要等停核回执才清，
        // 若收流延迟到工作线程，relay 能在间隙重开并读到拆核尾帧。
        StatsBridge.closeAll()
        unregisterStopReceiver()
        mainHandler.post { notification.close() }
        Thread({
            // 关 fd 必须排在关内核之前：内核的 tun 读写循环还持有它，先关服务会让那个循环
            // 拿着一个已经无效的 fd 空转到超时。
            runCatching { fileDescriptor?.close() }
            fileDescriptor = null
            DefaultNetworkMonitor.stop()
            val closed = runCatching {
                TransientLoginHost.closeMain(this) {
                    commandServer?.let { server ->
                        server.closeService()
                        server.close()
                    }
                    commandServer = null
                }
            }
            if (closed.isFailure) {
                commandServer?.setError("android: close service failed")
                // Keep the ownership claim: an unconfirmed close cannot permit a second state writer.
                closeFailed = true
                VpnBridge.finishStop("android: 内核关闭失败")
                return@Thread
            }
            state = ServiceState.Stopped
            // 结账排在 fd 关闭 + 内核关停**之后**：桥的回执语义是「隧道确实拆干净了」，
            // 提前结账会让 Rust 侧在 tun 还在的时候就去起第二个核。
            VpnBridge.finishStop()
            mainHandler.post { service.stopSelf() }
        }, "polaris-box-stop").start()
    }

    private fun registerStopReceiver() {
        if (receiverRegistered) return
        ContextCompat.registerReceiver(
            service,
            stopReceiver,
            IntentFilter(ServiceAction.SERVICE_STOP),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        receiverRegistered = true
    }

    private fun unregisterStopReceiver() {
        if (!receiverRegistered) return
        runCatching { service.unregisterReceiver(stopReceiver) }
        receiverRegistered = false
    }

    companion object {
        private const val TAG = "PolarisBoxService"
        private const val PROFILE_NAME = "Polaris"

        /** 外部（通知按钮、将来的 UI/命令面）请求停机的唯一入口。 */
        fun requestStop(context: Context) {
            context.sendBroadcast(Intent(ServiceAction.SERVICE_STOP).setPackage(context.packageName))
        }
    }
}
