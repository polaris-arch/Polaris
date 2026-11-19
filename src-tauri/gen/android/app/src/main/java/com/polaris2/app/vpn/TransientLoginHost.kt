package com.polaris2.app.vpn

import io.nekohasekai.libbox.CommandServer
import io.nekohasekai.libbox.CommandServerHandler
import io.nekohasekai.libbox.Libbox
import io.nekohasekai.libbox.OverrideOptions
import io.nekohasekai.libbox.SystemProxyStatus
import android.util.Log
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
        parseDirectories = ::tailscaleDirectoryClaims,
        requireSupported = SystemEndpointGuard::requireSupported,
        createEngine = { id, config, _, close -> LibboxEngine(id, config, close) },
        logFailure = { stage, failure, reason ->
            Log.w("PolarisLogin", "transient failure stage=$stage type=${failure.javaClass.simpleName} reason=$reason")
        },
    )

    fun <T> withMainConfig(owner: MainKernelAttempt<*>, config: String, allowed: () -> Boolean = { true }, action: () -> T): T =
        state.withMainConfig(owner, config, allowed, action)
    fun <T> withMainReload(
        owner: MainKernelAttempt<*>, config: String, allowed: () -> Boolean,
        joinsOriginal: (RuntimeTailscaleWriterScope) -> Boolean, action: () -> T,
    ): T = state.withMainReload(owner, config, allowed, joinsOriginal, action)
    fun closeMain(owner: Any, action: () -> Unit) = state.closeMain(owner, action)
    fun start(id: String, config: String, done: (StartFailure?) -> Unit) = state.start(id, config, done)
    fun close(id: String, done: (String?) -> Unit) = state.close(id, done)
    fun closeCoded(id: String, done: (AndroidNativeFailure?) -> Unit) = state.closeCoded(id, done)
    fun running(id: String): Boolean = state.running(id)
    fun closeTailscale(binding: JSONObject, done: (AndroidNativeFailure?) -> Unit) = state.closeTailscale(binding, done)

    fun validateRuntimeDirectories(owner: MainKernelAttempt<*>, directories: Set<String>, allowed: () -> Boolean): Boolean =
        state.validateRuntimeDirectories(owner, directories, allowed)
    fun claimRuntimeDirectories(
        owner: MainKernelAttempt<*>, scope: RuntimeTailscaleWriterScope, directories: Set<String>,
        retainScope: () -> Boolean, allowed: () -> Boolean,
    ): Boolean = state.claimRuntimeDirectories(owner, scope, directories, retainScope, allowed)
    fun releaseRuntimeDirectories(owner: MainKernelAttempt<*>, scope: RuntimeTailscaleWriterScope): Boolean =
        state.releaseRuntimeDirectories(owner, scope)
    fun hasRuntimeDirectoryClaims(owner: MainKernelAttempt<*>): Boolean = state.hasRuntimeDirectoryClaims(owner)
    fun observeRuntimeScope(
        owner: MainKernelAttempt<*>, incarnation: String, digest: String, allowed: (RuntimeTailscaleWriterScope) -> Boolean,
    ): RuntimeTailscaleWriterScope? = state.observeRuntimeScope(owner, incarnation, digest, allowed)

    internal fun tailscaleDirectoryClaims(config: String): TailscaleDirectoryClaims {
        val endpoints = JSONObject(config).optJSONArray("endpoints")
            ?: return TailscaleDirectoryClaims(emptySet())
        val directories = mutableSetOf<String>()
        var resolved = true
        for (index in 0 until endpoints.length()) {
            val endpoint = endpoints.optJSONObject(index) ?: continue
            if (endpoint.optString("type") != "tailscale") continue
            val path = endpoint.optString("state_directory")
            check(path.isNotBlank()) { "Android Tailscale 状态目录不可为空" }
            directories += File(path).canonicalPath
            val taildrop = endpoint.optString("taildrop_directory")
            // Core resolves its default and relative paths in its own working directory and expands variables.
            if (!File(path).isAbsolute || '$' in path || !File(taildrop).isAbsolute || '$' in taildrop) {
                resolved = false
            } else {
                directories += File(taildrop).canonicalPath
            }
        }
        return TailscaleDirectoryClaims(directories, resolved)
    }

    private class LibboxEngine(private val id: String, config: String,
        private val requestClose: () -> Unit) : TransientLoginHostState.Engine {
        private var config: String? = config
        private var server: CommandServer? = null
        private var network: TransientLoginNetwork? = null
        private var cache: File? = null
        private var store: AndroidTailscaleStoreCustody? = null
        private val cleanup = TransientHostCleanup(
            beginResolverClose = { network?.beginResolverClose() },
            closeService = { server?.let { server ->
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
            cache = requireAttemptCache(PolarisApplication.application.dataDir, id, cachePath)
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
            }
            finally { config = null }
        }

        override fun close() {
            try { cleanup.close() }
            finally { config = null }
        }
    }

    private val attemptId = Regex("tailscale-login-[A-Za-z0-9_-]+-[0-9]+")

    /** Match the cache Rust creates beside the config for this exact login instance. */
    internal fun requireAttemptCache(dataDir: File, id: String, path: String): File {
        check(attemptId.matches(id)) { "Android 登录缓存实例标识无效" }
        val relative = "polaris/$id.cache.db"
        val nominal = File(dataDir, relative).absoluteFile
        val expected = File(dataDir.canonicalFile, relative)
        val supplied = File(path)
        check((supplied == nominal || supplied == expected) && supplied.canonicalFile == expected && supplied.isFile) {
            "Android 登录缓存路径未隔离"
        }
        return expected
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
