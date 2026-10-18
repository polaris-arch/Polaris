package com.polaris2.app.vpn

import io.nekohasekai.libbox.CommandServer
import io.nekohasekai.libbox.CommandServerHandler
import io.nekohasekai.libbox.Libbox
import io.nekohasekai.libbox.OverrideOptions
import io.nekohasekai.libbox.SystemProxyStatus
import android.util.Log
import android.system.Os
import android.system.ErrnoException
import android.system.OsConstants
import java.io.File
import org.json.JSONObject
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/** Non-VPN login instances. The injected state machine is also the production Entry/claim owner. */
internal object TransientLoginHost {
    sealed class StartFailure(val message: String, val code: String? = null)
    class GeneralFailure(message: String, code: String? = null) : StartFailure(message, code)
    class SystemInterfaceFailure : StartFailure(SystemEndpointGuard.ERROR)
    private val worker = Executors.newSingleThreadExecutor { Thread(it, "polaris-login-host") }
    private val timer = Executors.newSingleThreadScheduledExecutor { Thread(it, "polaris-login-expiry") }
    private val state = TransientLoginHostState(
        ledger = AndroidNativeAdmissionGate.ledger,
        queue = { worker.execute(it) },
        schedule = { delay, action -> timer.schedule(action, delay, TimeUnit.MILLISECONDS); Unit },
        parseDirectories = ::stateDirectories,
        requireSupported = SystemEndpointGuard::requireSupported,
        createEngine = { id, config, directories, close -> LibboxEngine(id, config, directories, close) },
        logFailure = { stage, failure, reason ->
            Log.w("PolarisLogin", "transient failure stage=$stage type=${failure.javaClass.simpleName} reason=$reason")
        },
    )

    fun <T> withMainConfig(owner: MainKernelAttempt<*>, config: String, allowed: () -> Boolean = { true }, action: () -> T): T =
        state.withMainConfig(owner, config, allowed, action)
    fun closeMain(owner: Any, action: () -> Unit) = state.closeMain(owner, action)
    fun start(id: String, config: String, done: (StartFailure?) -> Unit) = state.start(id, config, done)
    fun close(id: String, done: (String?) -> Unit) = state.close(id, done)
    fun closeCoded(id: String, done: (AndroidNativeFailure?) -> Unit) = state.closeCoded(id, done)
    fun running(id: String): Boolean = state.running(id)
    fun closeTailscale(binding: JSONObject, done: (AndroidNativeFailure?) -> Unit) = state.closeTailscale(binding, done)

    private fun stateDirectories(config: String): Set<String> {
        val endpoints = JSONObject(config).optJSONArray("endpoints") ?: return emptySet()
        return (0 until endpoints.length()).mapNotNull { index ->
            val endpoint = endpoints.optJSONObject(index) ?: return@mapNotNull null
            if (endpoint.optString("type") != "tailscale") return@mapNotNull null
            val path = endpoint.optString("state_directory")
            check(path.isNotBlank()) { "Android Tailscale 状态目录不可为空" }
            File(path).canonicalPath
        }.toSet()
    }

    private class LibboxEngine(id: String, config: String, private val stateDirectories: Set<String>,
        private val requestClose: () -> Unit) : TransientLoginHostState.Engine {
        private val closeTimeout = DebugTransientCloseTimeout.process.Target("login", id)
        private var config: String? = config
        private var server: CommandServer? = null
        private var network: TransientLoginNetwork? = null
        private var cache: File? = null
        private var store: AndroidTailscaleStoreCustody? = null
        private val cleanup = TransientHostCleanup(
            beginResolverClose = { network?.beginResolverClose() },
            closeService = { closeTimeout.beforeClose(); server?.let { server ->
                try { server.closeService() }
                finally { store?.closed { AndroidTailscaleStoreCustody.export(server) } }
            } },
            closeServer = { server?.let { server -> server.close() } },
            closeNetwork = { network?.close() },
            nativeClosed = { server = null },
            networkClosed = { network = null },
            closeCache = {
                cache?.let { check(!it.exists() || it.delete()) { "Android 登录缓存清理失败" } }
                cache = null
            },
        )

        override fun bindTailscaleStore(store: AndroidTailscaleStoreCustody) { check(this.store == null); this.store = store }

        override fun prepare(validationTicket: AndroidNativeAdmission.Ticket, stage: (String) -> Unit, cancelled: () -> Boolean) {
            val value = checkNotNull(config)
            stage("setup")
            PolarisApplication.ensureSetup()
            check(!Libbox.hasTunInbound(value)) { "Android 独立登录不允许创建 VPN 隧道" }
            stage("check")
            AndroidNativeValidation.check(validationTicket, value)
            stage("cache")
            val cachePath = JSONObject(value).optJSONObject("experimental")?.optJSONObject("cache_file")?.optString("path").orEmpty()
            val createdCache = File(cachePath).canonicalFile
            check(createdCache.parent in stateDirectories && createdCache.name.matches(Regex("login-cache-[0-9]+\\.db"))) { "Android 登录缓存路径未隔离" }
            // Cache Initialize precedes endpoint Initialize; a first login has no TS directory yet.
            ensurePrivateDirectory(requireNotNull(createdCache.parentFile))
            cache = createdCache
            val createdNetwork = TransientLoginNetwork()
            network = createdNetwork
            stage("network")
            createdNetwork.start()
            check(!cancelled()) { "Android 登录请求已取消" }
            // The factory disables global command socket/snapshot/power reports inside libbox.
            stage("factory")
            server = Libbox.newTransientCommandServer(LoginHandler(requestClose), createdNetwork)
        }

        override fun start() {
            try {
                val original = checkNotNull(server)
                checkNotNull(store).invoke(checkNotNull(config), { AndroidTailscaleStoreCustody.export(original) }) {
                    original.startOrReloadService(checkNotNull(config), OverrideOptions())
                }
                closeTimeout.started()
            }
            finally { config = null }
        }

        override fun close() {
            try { cleanup.close() }
            finally { config = null }
        }
    }

    private fun ensurePrivateDirectory(directory: File) {
        if (directory.isDirectory) return // Keep existing permissions and user state.
        check(!directory.exists()) { "Android 登录缓存父路径不是目录" }
        directory.parentFile?.let { ensurePrivateDirectory(it) }
        try { Os.mkdir(directory.path, 448) } // 0700, supported by every app minSdk.
        catch (error: ErrnoException) {
            if (error.errno != OsConstants.EEXIST || !directory.isDirectory) throw error
        }
    }

    private class LoginHandler(private val requestClose: () -> Unit) : CommandServerHandler {
        override fun serviceStop() { requestClose() }
        override fun serviceReload() { error("Android 独立登录实例不能重载主代理") }
        override fun getSystemProxyStatus() = SystemProxyStatus().apply { available = false; enabled = false }
        override fun setSystemProxyEnabled(enabled: Boolean) { error("Android 独立登录不允许设置系统代理") }
        override fun triggerNativeCrash() { error("Android 独立登录不提供崩溃指令") }
        override fun writeDebugMessage(message: String?) { /* Auth URLs/secrets stay in the STATUS stream. */ }
        override fun connectSSHAgent(): Int = -1
    }
}
