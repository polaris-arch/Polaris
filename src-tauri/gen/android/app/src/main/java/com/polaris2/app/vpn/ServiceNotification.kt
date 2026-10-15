// 结构参考 sing-box-for-android 的 app/src/main/java/io/nekohasekai/sfa/bg/ServiceNotification.kt
//（前台通知的频道/构建方式、用 CommandClient 订阅 status 刷速率、屏幕熄灭时断开订阅省电）。
// Copyright (C) 2022 by nekohasekai <contact-sagernet@sekai.icu>
// 该项目按 GNU General Public License v3（或更新版本）分发，本文件因此继承 GPLv3。
// 与上游的差异：上游用自建的 CommandClient 封装 + kotlinx-coroutines；本仓直接用
// `Libbox.newCommandClient`，回调线程由 gomobile 提供，UI 更新自己切到主线程。

package com.polaris2.app.vpn

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import com.polaris2.app.MainActivity
import com.polaris2.app.R
import io.nekohasekai.libbox.CommandClient
import io.nekohasekai.libbox.CommandClientHandler
import io.nekohasekai.libbox.CommandClientOptions
import io.nekohasekai.libbox.ConnectionEvents
import io.nekohasekai.libbox.Libbox
import io.nekohasekai.libbox.LogIterator
import io.nekohasekai.libbox.OutboundGroupItemIterator
import io.nekohasekai.libbox.OutboundGroupIterator
import io.nekohasekai.libbox.StatusMessage
import io.nekohasekai.libbox.StringIterator

/** Serializes callback publication with invalidation, like StatsBridge's service epoch. */
internal class ServiceNotificationEpoch {
    private var generation = 0L
    private var active = false

    @Synchronized fun begin(): Long {
        generation += 1
        active = true
        return generation
    }

    @Synchronized fun invalidate(): Long {
        generation += 1
        active = false
        return generation
    }

    @Synchronized fun runIfCurrent(token: Long, action: () -> Unit): Boolean {
        if (!active || generation != token) return false
        action()
        return true
    }

    @Synchronized fun runIfGeneration(token: Long, action: () -> Unit): Boolean {
        if (generation != token) return false
        action()
        return true
    }
}

/**
 * 前台服务通知。
 *
 * 这个通知不是「合规摆设」：Android 8+ 持有 VpnService 必须常驻一条前台通知，它是移动端**唯一**
 * 一处始终可见、始终可点的控制入口 —— 桌面托盘承载的三件事（连接开关 / 当前在用什么 / 回到主界面）
 * 在这里都有归宿：
 *   - 标题 = 状态（正在启动 / 已连接 / 正在停止），对应托盘图标的三态；
 *   - 正文 = 当前配置名 + 实时上下行速率，对应托盘 tooltip；
 *   - action「断开」= 托盘的连接开关（关的那一侧）；
 *   - 点击通知本体 = 回到主界面（托盘的「显示窗口」）。
 *
 * 还缺的一件是「切换节点」：它需要出站分组与节点列表，那要从 `CommandClient` 的 group 通道取，
 * 而选中动作要回到 Rust 侧的选择器状态（K4 的命令接线）。在那之前放一个只能切一半的按钮，
 * 比不放更糟 —— 用户会以为切了。
 */
class ServiceNotification(private val service: Service) {
    private val handler = Handler(Looper.getMainLooper())
    private val epoch = ServiceNotificationEpoch()
    private var commandClient: CommandClient? = null
    private var screenReceiverRegistered = false

    @Volatile private var state: ServiceState = ServiceState.Starting
    private var profileName: String = ""
    private var trafficLine: String? = null

    private val screenReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) = synchronized(epoch) {
            if (state != ServiceState.Started) return@synchronized
            // 熄屏时没人看得到速率，却每秒唤醒一次进程去刷它。上游同款处置。
            when (intent.action) {
                Intent.ACTION_SCREEN_ON -> connectStatusClient()
                Intent.ACTION_SCREEN_OFF -> disconnectStatusClient()
            }
            Unit
        }
    }

    private val builder by lazy {
        NotificationCompat.Builder(service, CHANNEL_ID)
            .setShowWhen(false)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setContentIntent(
                PendingIntent.getActivity(
                    service,
                    0,
                    Intent(service, MainActivity::class.java)
                        .setFlags(Intent.FLAG_ACTIVITY_REORDER_TO_FRONT),
                    PENDING_FLAGS,
                ),
            )
            .addAction(
                NotificationCompat.Action.Builder(
                    0,
                    service.getString(R.string.notification_action_disconnect),
                    PendingIntent.getBroadcast(
                        service,
                        0,
                        Intent(ServiceAction.SERVICE_STOP).setPackage(service.packageName),
                        PENDING_FLAGS,
                    ),
                ).build(),
            )
    }

    /** 首次调用会把服务提到前台；之后每次只是更新同一条通知。 */
    fun show(state: ServiceState, profileName: String) = synchronized(epoch) {
        disconnectStatusClient()
        epoch.begin()
        trafficLine = null
        this.state = state
        this.profileName = profileName
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            PolarisApplication.notification.createNotificationChannel(
                NotificationChannel(
                    CHANNEL_ID,
                    service.getString(R.string.notification_channel_service),
                    NotificationManager.IMPORTANCE_LOW,
                ),
            )
        }
        // startForeground 必须在 onStartCommand 后 5 秒内调到，否则系统抛 ForegroundServiceDidNotStartInTimeException。
        // 所以状态刚变成 Starting 就先立一条，内容随后再刷，而不是等内核起完。
        service.startForeground(NOTIFICATION_ID, build())
    }

    fun onStarted() = synchronized(epoch) {
        if (state != ServiceState.Starting) return@synchronized
        state = ServiceState.Started
        PolarisApplication.notification.notify(NOTIFICATION_ID, build())
        registerScreenReceiver()
        connectStatusClient()
    }

    /** Stop invalidates immediately, before BoxService queues foreground removal. */
    fun invalidateForStop(): Long = synchronized(epoch) {
        state = ServiceState.Stopping
        trafficLine = null
        val token = epoch.invalidate()
        handler.removeCallbacksAndMessages(null)
        token
    }

    fun close(expectedEpoch: Long) {
        epoch.runIfGeneration(expectedEpoch) {
            state = ServiceState.Stopped
            disconnectStatusClient()
            if (screenReceiverRegistered) {
                runCatching { service.unregisterReceiver(screenReceiver) }
                screenReceiverRegistered = false
            }
            ServiceCompat.stopForeground(service, ServiceCompat.STOP_FOREGROUND_REMOVE)
        }
    }

    private fun build(): android.app.Notification {
        val title = service.getString(
            when (state) {
                ServiceState.Starting -> R.string.notification_state_starting
                ServiceState.Started -> R.string.notification_state_started
                ServiceState.Stopping -> R.string.notification_state_stopping
                ServiceState.Stopped -> R.string.notification_state_stopped
            },
        )
        val body = listOfNotNull(profileName.takeIf { it.isNotBlank() }, trafficLine).joinToString(" · ")
        return builder.setContentTitle(title).setContentText(body).build()
    }

    private fun registerScreenReceiver() {
        if (screenReceiverRegistered) return
        ContextCompat.registerReceiver(
            service,
            screenReceiver,
            IntentFilter().apply {
                addAction(Intent.ACTION_SCREEN_ON)
                addAction(Intent.ACTION_SCREEN_OFF)
            },
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        screenReceiverRegistered = true
    }

    private fun connectStatusClient(): Unit = synchronized(epoch) {
        if (state != ServiceState.Started || commandClient != null) return@synchronized
        val token = epoch.begin()
        val options = CommandClientOptions().apply {
            addCommand(Libbox.CommandStatus)
            statusInterval = STATUS_INTERVAL_NANOS
        }
        val client = Libbox.newCommandClient(StatusHandler(token), options)
        commandClient = client
        // Each client captures its own epoch, including screen-off/reconnect cycles.
        runCatching { client.connect() }.onFailure { Log.w(TAG, "订阅状态失败", it) }
        Unit
    }

    private fun disconnectStatusClient() {
        val client = synchronized(epoch) {
            epoch.invalidate()
            handler.removeCallbacksAndMessages(null)
            trafficLine = null
            commandClient.also { commandClient = null }
        }
        runCatching { client?.disconnect() }
    }

    private inner class StatusHandler(private val token: Long) : CommandClientHandler {
        override fun connected() {}

        override fun disconnected(message: String?) {
            handler.post {
                epoch.runIfCurrent(token) { trafficLine = null }
            }
        }

        override fun writeStatus(message: StatusMessage) {
            val line = "↑ ${Libbox.formatBytes(message.uplink)}/s  ↓ ${Libbox.formatBytes(message.downlink)}/s"
            handler.post {
                epoch.runIfCurrent(token) {
                    if (state == ServiceState.Started) {
                        trafficLine = line
                        PolarisApplication.notification.notify(NOTIFICATION_ID, build())
                    }
                }
            }
        }

        // 以下通道本通知不订阅（CommandClientOptions 只 addCommand(CommandStatus)），
        // 但接口是全量的，必须给出空实现。
        override fun clearLogs() {}

        override fun initializeClashMode(modeList: StringIterator?, currentMode: String?) {}

        override fun setDefaultLogLevel(level: Int) {}

        override fun updateClashMode(newMode: String?) {}

        override fun writeConnectionEvents(events: ConnectionEvents?) {}

        override fun writeGroups(groups: OutboundGroupIterator?) {}

        override fun writeLogs(messageList: LogIterator?) {}

        override fun writeOutbounds(outbounds: OutboundGroupItemIterator?) {}
    }

    companion object {
        private const val TAG = "PolarisNotification"
        private const val NOTIFICATION_ID = 1
        private const val CHANNEL_ID = "polaris-service"
        private const val STATUS_INTERVAL_NANOS = 1_000_000_000L
        private val PENDING_FLAGS =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0
    }
}
