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
import io.nekohasekai.libbox.Libbox
import io.nekohasekai.libbox.Notification
import io.nekohasekai.libbox.OverrideOptions
import io.nekohasekai.libbox.PlatformInterface
import io.nekohasekai.libbox.SystemProxyStatus
import io.nekohasekai.libbox.TunOptions
import java.util.concurrent.TimeUnit

/** Keep native error details out of logcat while retaining the failing close component. */
internal fun safeCloseFailureComponent(failure: Throwable): String {
    val message = failure.message.orEmpty()
    val components = listOf(
        "service", "inbound", "certificate-provider", "endpoint", "outbound", "router",
        "connection", "dns-router", "dns-transport", "network", "http-client", "logger",
    ).filter { message.contains("close $it", ignoreCase = true) }
    return components.joinToString("+").ifEmpty { "unknown" }
}

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
    /** A late openTun after Stop must close its fd before returning to native code. */
    internal fun installTun(attempt: MainKernelAttempt<CommandServer>, descriptor: ParcelFileDescriptor, scope: TunScope): Int {
        val previous = try {
            synchronized(this) {
                check(mainAttempt === attempt && !attempt.revoked &&
                    (state == ServiceState.Starting || state == ServiceState.Started)) {
                    "旧主核已撤销，拒收迟到的 VPN fd"
                }
                MainKernelAttemptRegistry.installTun(attempt, descriptor, scope)
            }
        } catch (error: Throwable) {
            runCatching { descriptor.close() }
            throw error
        }
        runCatching { previous?.close() }
        return descriptor.fd
    }

    @Volatile
    var state: ServiceState = ServiceState.Stopped
        private set

    private val mainHandler = Handler(Looper.getMainLooper())
    private val notification = ServiceNotification(service)
    @Volatile private var commandServer: CommandServer? = null
    @Volatile private var mainAttempt: MainKernelAttempt<CommandServer>? = null
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
        val (attempt, stopUnowned) = synchronized(this) {
            if (state != ServiceState.Stopped || mainAttempt != null) Pair(null, false)
            else {
                val generation = SystemStart.generation()
                val systemRunId = java.util.UUID.randomUUID().toString()
                val admission = LegacySystemStartFence.admitWithDecision {
                    val request = VpnBridge.currentStartRequest()
                    val next = MainKernelAttempt<CommandServer>(
                        generation,
                        request?.runId ?: systemRunId,
                    )
                    if (!MainKernelAttemptRegistry.isVacant() ||
                        (request == null && !VpnBridge.beginSystemStart())) null
                    else {
                        check(MainKernelAttemptRegistry.claim(next) { stopService(next) }) {
                            "主核准入锁内 registry 归属发生变化"
                        }
                        mainAttempt = next
                        state = ServiceState.Starting
                        next
                    }
                }
                Pair(admission.value, shouldStopSelfAfterFenceRejection(
                    admission.rejectedByFence, state, mainAttempt != null,
                ))
            }
        }
        if (attempt == null) {
            if (stopUnowned) {
                // startForegroundService already committed the 5s FGS deadline.
                // This instance owns no attempt, so stop it before that deadline.
                Log.i(TAG, "迁移屏障拒绝起核意图，停止无主前台服务")
                service.stopSelf()
                return Service.START_NOT_STICKY
            }
            // An existing attempt owns its bridge reply. A duplicate system intent
            // cannot reject that pending Start or clear its config.
            Log.i(TAG, "忽略重复起核意图，服务处于 $state 状态")
            return Service.START_NOT_STICKY
        }
        // 先立前台通知：startForeground 的 5 秒窗口从 onStartCommand 起算，而内核起来要秒级。
        try {
            notification.show(ServiceState.Starting, PROFILE_NAME)
            registerStopReceiver()
            // A Stop broadcast sent before receiver registration has no target.
            // Recheck the bridge after registration and take over that pending Stop.
            if (VpnBridge.isStopping() || !isStarting(attempt)) {
                attempt.skipPreparation()
                // An exact barrier close may have run before registration. Its
                // first Stop could not unregister a receiver that did not exist.
                unregisterStopReceiver()
                stopService(attempt)
                return Service.START_NOT_STICKY
            }
            Thread({ startKernel(attempt) }, "polaris-box-start").start()
        } catch (error: Throwable) {
            attempt.skipPreparation()
            synchronized(this) {
                if (mainAttempt === attempt) VpnBridge.finishStart(error.message ?: "Android 起核线程创建失败")
            }
            stopService(attempt)
        }
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

    private fun isStarting(attempt: MainKernelAttempt<CommandServer>): Boolean = synchronized(this) {
        mainAttempt === attempt && !attempt.revoked && state == ServiceState.Starting
    }

    private fun startKernel(attempt: MainKernelAttempt<CommandServer>) {
        // 桥交来的那一份（经桥起核）；`null` = 系统发起的起核，配置读 Rust 落盘的同一份文件。
        val bridgeConfig = VpnBridge.currentConfig()
        try {
            check(isStarting(attempt)) { "旧起核尝试已撤销" }
            val config = bridgeConfig ?: SystemStart.load(service)
            if (bridgeConfig != null) {
                val request = VpnBridge.currentStartRequest()
                check(request != null && request.runId == attempt.runId &&
                    SystemStart.sha256(config.toByteArray(Charsets.UTF_8)) == request.configDigest) {
                    "android: 起核配置摘要或 runId 与主核 attempt 不一致"
                }
            }
            // This slice has no managed Start admission yet. A marker blocks every legacy bridge start.
            if (bridgeConfig != null) SystemStart.requireLegacyAllowed(service)
            SystemEndpointGuard.requireSupported(config)
            PolarisApplication.ensureSetup()
            TransientSpeedtestHost.withMainStart(attempt, { isStarting(attempt) }) {
                TransientLoginHost.withMainConfig(attempt, config, { isStarting(attempt) }) {
                    check(isStarting(attempt)) { "起核已被停核接管" }
                    DefaultNetworkMonitor.start()
                    check(isStarting(attempt)) { "起核已被停核接管" }
                    val tunOpener = platformInterface as? PolarisVpnService
                        ?: error("android: 主核没有绑定 attempt 的 TUN 载体")
                    val boundPlatform = object : PlatformInterface by platformInterface {
                        override fun openTun(options: TunOptions): Int = tunOpener.openTun(attempt, options)
                    }
                    val server = Libbox.newStrictCommandServer(AttemptHandler(attempt, this), boundPlatform)
                    attempt.publish(server)
                    synchronized(this) { if (mainAttempt === attempt) commandServer = server }
                    check(isStarting(attempt)) { "起核已被停核接管" }
                    server.start()
                    // No login instance may hold this Tailscale state directory during main startup.
                    check(isStarting(attempt)) { "起核已被停核接管" }
                    SystemStart.requireLegacyAllowed(service)
                    server.startOrReloadService(config, OverrideOptions())
                    SystemStart.requireLegacyAllowed(service)
                }
            }
            val acknowledged = synchronized(this) {
                // stopService 持同一把锁：停核已接管时不可重新放开命令流。
                check(mainAttempt === attempt && !attempt.revoked && state == ServiceState.Starting) { "起核已被停核接管" }
                attempt.acknowledgeStart()
                state = ServiceState.Started
                StatsBridge.activateAll()
                // The token check and bridge acknowledgement share Stop's lock.
                VpnBridge.finishStart(null, attempt = attempt)
            }
            if (!acknowledged) {
                // The native service may already be running. Keep its registry owner
                // until this exact attempt's close completes; never publish it as ready.
                stopService(attempt)
                return
            }
            mainHandler.post {
                synchronized(this) {
                    if (mainAttempt === attempt && MainKernelAttemptRegistry.isCurrent(attempt) &&
                        !attempt.revoked && state == ServiceState.Started) {
                        notification.onStarted()
                    }
                }
            }
            Log.i(TAG, "内核已启动")
            // 经桥起核成功 ⇒ 记下这一份，此后系统发起的起核才有配置可用。排在结账之后：它要读一次
            // 盘并算摘要，不该拖慢 Rust 侧的就绪门。失败只影响「系统起核可用性」，不影响本次连接。
            if (bridgeConfig != null) {
                runCatching { SystemStart.remember(service, bridgeConfig, attempt.systemStartGeneration) {
                    mainAttempt === attempt && MainKernelAttemptRegistry.isCurrent(attempt) &&
                        !attempt.revoked && state == ServiceState.Started
                } }
                    .onFailure { Log.e(TAG, "记录起核配置失败，系统发起的起核将不可用", it) }
            }
        } catch (e: Throwable) {
            attempt.skipPreparation()
            // 🔴 这条 catch 此前只 Log.e + stopService()，Rust 侧什么都收不到 —— 那正是
            // 「静默没起来」的现场。先结账（把内核原话带回去），再拆自己。
            Log.e(TAG, "内核启动失败", e)
            // 经桥起核失败：盘上那份已是这次失败的尝试，旧摘要作废。系统起核失败**不撤**准入：
            // 用户的意图没变（仍是「连着」），下一次系统重试 / 用户手动连接自然会覆盖。
            val current = synchronized(this) {
                if (mainAttempt !== attempt || attempt.revoked || state != ServiceState.Starting) false
                else {
                    if (bridgeConfig != null) SystemStart.forget(service, "经桥起核失败")
                    VpnBridge.finishStart(
                        e.message ?: e.toString(),
                        if (e is SystemEndpointGuard.Unsupported) SystemEndpointGuard.ERROR else PolarisVpnPlugin.ERR_STARTUP_FAILED,
                    )
                    true
                }
            }
            if (current) stopService(attempt)
        }
    }

    // ── CommandServerHandler ────────────────────────────────────────────────────

    private inner class AttemptHandler(
        private val attempt: MainKernelAttempt<CommandServer>,
        delegate: CommandServerHandler,
    ) : CommandServerHandler by delegate {
        override fun serviceStop() {
            stopService(attempt)
        }

        override fun serviceReload() {
            this@BoxService.serviceReload(attempt)
        }
    }

    override fun serviceStop() {
        stopService()
    }

    override fun serviceReload() { serviceReload(null) }

    private fun serviceReload(expectedAttempt: MainKernelAttempt<CommandServer>?) {
        val (attempt, server) = synchronized(this) {
            val current = mainAttempt?.takeIf {
                (expectedAttempt == null || it === expectedAttempt) && !it.revoked && state == ServiceState.Started
            } ?: return
            Pair(current, commandServer ?: return)
        }
        // 系统发起的核没有桥配置 ⇒ 与起核同源，读 Rust 落盘的那一份。
        val config = VpnBridge.currentConfig()
            ?: runCatching { SystemStart.load(service) }.getOrElse {
                setReloadError(attempt, server, it)
                return
            }
        try {
            SystemEndpointGuard.requireSupported(config)
        } catch (error: Exception) {
            setReloadError(attempt, server, error)
            return
        }
        runCatching { TransientSpeedtestHost.withMainStart(attempt, { synchronized(this) {
            mainAttempt === attempt && !attempt.revoked && state == ServiceState.Started && commandServer === server
        } }) {
            TransientLoginHost.withMainConfig(attempt, config, { synchronized(this) {
                mainAttempt === attempt && !attempt.revoked && state == ServiceState.Started && commandServer === server
            } }) {
                SystemStart.requireLegacyAllowed(service)
                server.startOrReloadService(config, OverrideOptions())
                SystemStart.requireLegacyAllowed(service)
            }
        } }
            .onFailure {
                Log.e(TAG, "重载失败", it)
                setReloadError(attempt, server, it)
            }
    }

    private fun setReloadError(attempt: MainKernelAttempt<CommandServer>, server: CommandServer, error: Throwable) {
        synchronized(this) {
            if (mainAttempt === attempt && !attempt.revoked && commandServer === server) {
                runCatching { server.setError("android: reload: ${error.message}") }
            }
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
    private fun stopService(expectedAttempt: MainKernelAttempt<CommandServer>? = null) {
        val attempt = mainAttempt
        if (expectedAttempt != null && attempt !== expectedAttempt) return
        if (attempt != null && !MainKernelAttemptRegistry.isCurrent(attempt)) return
        if (state == ServiceState.Stopped && attempt == null) {
            return
        }
        // A pending close owns this attempt. A second stop cannot start another native
        // close or release its claims while the first job is still running.
        if (state == ServiceState.Stopping && !closeFailed) return
        if (attempt == null) {
            return
        }
        if (attempt.closed.isDone) {
            val failure = attempt.closed.getNow(null)
            if (failure != null) VpnBridge.finishStop("android: 内核关闭失败")
            return
        }
        val firstStop = state != ServiceState.Stopping
        closeFailed = false
        state = ServiceState.Stopping
        val detachedTun = attempt.revokeAndDetachTun()
        StatsBridge.closeAll()
        unregisterStopReceiver()
        if (firstStop) {
            mainHandler.post {
                synchronized(this) {
                    if (MainKernelAttemptRegistry.isCurrent(attempt) ||
                        (mainAttempt == null && state == ServiceState.Stopped && MainKernelAttemptRegistry.isVacant())) {
                        notification.close()
                    }
                }
            }
            attempt.closed.whenComplete { failure, error ->
                onAttemptClosed(attempt, error ?: failure)
            }
            attempt.closeOnce { server ->
                var closeStage = "network-monitor"
                try {
                    DefaultNetworkMonitor.stop()
                    closeStage = "speedtest-ownership"
                    TransientSpeedtestHost.closeMain(attempt) {
                        // Start/Reload holds this exact attempt's operationLock. A
                        // pre-fence call already past the final admission check must
                        // leave native code before terminal Close can release owner.
                        synchronized(attempt.operationLock) {
                            closeStage = "login-ownership"
                            TransientLoginHost.closeMain(attempt) {
                                // Go's strict terminal close joins Start/OpenTun before this Java
                                // descriptor can be released; OpenInterface duplicates it afterwards.
                                closeStage = "native-service"
                                try { server?.closeService() }
                                finally { runCatching { detachedTun?.close() } }
                                closeStage = "native-command"
                                server?.close()
                            }
                        }
                    }
                } catch (failure: Throwable) {
                    // Native errors may contain a node address or config value. Log only
                    // fixed diagnostic labels; preserve the original failure for ownership.
                    Log.e(TAG, "主核关闭失败 stage=$closeStage component=${safeCloseFailureComponent(failure)} type=${failure.javaClass.simpleName}")
                    throw failure
                }
            }
        }
        Thread({
            try {
                attempt.closed.get(8, TimeUnit.SECONDS)
            } catch (_: java.util.concurrent.TimeoutException) {
                synchronized(this) {
                    if (mainAttempt === attempt && state == ServiceState.Stopping && !attempt.closed.isDone) {
                        closeFailed = true
                        VpnBridge.finishStop("android: 主核 cleanupUnknown，关闭仍在进行")
                    }
                }
            } catch (_: Exception) {
                // onAttemptClosed reports confirmed native errors.
            }
        }, "polaris-main-close-timeout").start()
    }

    private fun onAttemptClosed(attempt: MainKernelAttempt<CommandServer>, failure: Throwable?) {
        var deliverStop: (() -> Unit)? = null
        synchronized(this) {
            if (mainAttempt !== attempt || !MainKernelAttemptRegistry.isCurrent(attempt)) return
            if (failure != null) {
                runCatching { commandServer?.setError("android: close service failed") }
                closeFailed = true
                VpnBridge.finishStop("android: 内核关闭失败")
                return
            }
            // The old attempt cannot settle a later bridge: new admission remains
            // forbidden until this exact close result is acknowledged and cleared.
            check(MainKernelAttemptRegistry.completeAfterClose(attempt) {
                deliverStop = VpnBridge.takeFinishStop()
                commandServer = null
                mainAttempt = null
                state = ServiceState.Stopped
                closeFailed = false
            }) { "主核关闭回执与进程所有权不一致" }
        }
        deliverStop?.invoke()
        mainHandler.post {
            synchronized(this) {
                if (mainAttempt == null && state == ServiceState.Stopped && MainKernelAttemptRegistry.isVacant()) {
                    service.stopSelf()
                }
            }
        }
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
