package com.polaris2.app.vpn

import android.util.Log
import io.nekohasekai.libbox.CommandServer
import io.nekohasekai.libbox.CommandServerHandler
import io.nekohasekai.libbox.Libbox
import io.nekohasekai.libbox.OverrideOptions
import io.nekohasekai.libbox.SystemProxyStatus
import org.json.JSONObject

/** A stopped-VPN speedtest owns a separate libbox server and never touches the main service. */
internal object TransientSpeedtestHost {
    private val sessions = TransientSpeedtestSessions(logFailure = {
        Log.w("PolarisSpeedtest", "native lifecycle failed: ${it.javaClass.simpleName}")
    })

    /** Final guard before libbox construction; Rust checks the same boundary independently. */
    internal fun validateConfig(config: String) {
        try {
            val root = JSONObject(config)
            check(!root.has("experimental"))
            val inbounds = root.getJSONArray("inbounds")
            check(inbounds.length() in 1..512)
            val ports = mutableSetOf<Int>()
            val tags = mutableSetOf<String>()
            var password: String? = null
            for (index in 0 until inbounds.length()) {
                val inbound = inbounds.getJSONObject(index)
                check(inbound.getString("type") == "http")
                check(inbound.getString("listen") == "127.0.0.1")
                val port = inbound.getInt("listen_port")
                check(port in 1..65535 && ports.add(port))
                check(tags.add(inbound.getString("tag")))
                val users = inbound.getJSONArray("users")
                check(users.length() == 1)
                val user = users.getJSONObject(0)
                check(user.getString("username") == "polaris-temp")
                val secret = user.getString("password")
                check(secret.length >= 32)
                check(password == null || password == secret)
                password = secret
            }
            val endpoints = root.optJSONArray("endpoints")
            if (root.has("endpoints")) check(endpoints != null)
            if (endpoints != null) for (index in 0 until endpoints.length()) {
                val endpoint = endpoints.getJSONObject(index)
                check(endpoint.getString("type") != "tailscale")
            }
            val outbounds = root.optJSONArray("outbounds")
            if (root.has("outbounds")) check(outbounds != null)
            if (outbounds != null) for (index in 0 until outbounds.length()) {
                check(outbounds.getJSONObject(index).getString("type") != "tailscale")
            }
            SystemEndpointGuard.requireSupported(config)
            // Every admitted inbound is HTTP, so no TUN can be constructed.
        } catch (_: Exception) {
            // org.json errors can contain the source JSON and its one-time password.
            throw IllegalArgumentException("Android 临时测速配置被拒绝")
        }
    }

    fun start(id: String, config: String, done: (String?) -> Unit) {
        try {
            AndroidNativeValidation.enqueue({ Thread(it, "polaris-speedtest-check").start() }) { validationTicket ->
                try { validateConfig(config) } catch (_: Exception) {
                    AndroidNativeValidation.cancelBeforeBirth(validationTicket)
                    done("Android 临时测速配置被拒绝")
                    return@enqueue
                }
                try { sessions.start(id, LibboxEngine(id, config, validationTicket)) { failure ->
                    AndroidNativeValidation.cancelBeforeBirth(validationTicket)
                    done(failure)
                } } catch (_: Throwable) {
                    AndroidNativeValidation.cancelBeforeBirth(validationTicket)
                    done("Android 临时测速启动失败")
                }
            }
        } catch (_: Throwable) { done("Android 临时测速原生准入已关闭") }
    }

    fun close(id: String, done: (String?) -> Unit) = sessions.close(id, done)
    fun status(id: String): String = sessions.status(id)
    fun <T> withMainStart(owner: Any, allowed: () -> Boolean, action: () -> T): T =
        sessions.withMainStart(owner, allowed, action)
    fun closeMain(owner: Any, action: () -> Unit) = sessions.closeMain(owner, action)

    private class LibboxEngine(private val id: String, private val config: String,
        private val validationTicket: AndroidNativeAdmission.Ticket) : TransientSpeedtestSessions.Engine {
        private var network: TransientLoginNetwork? = null
        private var server: CommandServer? = null

        override fun prepare() {
            PolarisApplication.ensureSetup()
            AndroidNativeValidation.check(validationTicket, config)
            val created = TransientLoginNetwork()
            network = created
            created.start()
            server = Libbox.newTransientCommandServer(SpeedtestHandler(id), created)
        }

        override fun start() {
            requireNotNull(server).startOrReloadService(config, OverrideOptions())
        }

        override fun close() {
            var failure: Throwable? = null
            server?.let { value ->
                runCatching { value.closeService() }.onFailure { failure = it }
                runCatching { value.close() }.onFailure { if (failure == null) failure = it }
            }
            runCatching { network?.close() }.onFailure { if (failure == null) failure = it }
            failure?.let { throw it }
        }
    }

    private class SpeedtestHandler(private val id: String) : CommandServerHandler {
        override fun serviceStop() { close(id) {} }
        override fun serviceReload() { error("Android 临时测速不支持重载") }
        override fun getSystemProxyStatus() = SystemProxyStatus().apply { available = false; enabled = false }
        override fun setSystemProxyEnabled(enabled: Boolean) { error("Android 临时测速不允许系统代理") }
        override fun triggerNativeCrash() { error("Android 临时测速不提供崩溃指令") }
        override fun writeDebugMessage(message: String?) {}
        override fun connectSSHAgent(): Int = -1
    }
}
