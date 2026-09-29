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

/** Non-VPN login instances, keyed by a Rust-generated attempt identity. Never uses VpnBridge or command.sock. */
internal object TransientLoginHost {
    private const val MAX_INSTANCES = 8
    sealed class StartFailure(val message: String)
    class GeneralFailure(message: String) : StartFailure(message)
    class SystemInterfaceFailure : StartFailure(SystemEndpointGuard.ERROR)
    private val worker = Executors.newSingleThreadExecutor { Thread(it, "polaris-login-host") }
    private val timer = Executors.newSingleThreadScheduledExecutor { Thread(it, "polaris-login-expiry") }
    private class Entry(val id: String, val stateDirectories: Set<String>) {
        @Volatile var cancelled = false
        @Volatile var running = false
        var server: CommandServer? = null
        var network: TransientLoginNetwork? = null
        var disposed = false
        var cache: File? = null
    }
    private val entries = mutableMapOf<String, Entry>()
    private val ownershipLock = Any()
    private val mainClaims = mutableMapOf<Any, Set<String>>()
    private val mainClaimRevisions = mutableMapOf<Any, Long>()

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

    /** Claims precede core startup and survive failures until its actual close succeeds. */
    fun <T> withMainConfig(owner: MainKernelAttempt<*>, config: String, allowed: () -> Boolean = { true }, action: () -> T): T = synchronized(owner.operationLock) {
        val directories = stateDirectories(config)
        val revision = synchronized(ownershipLock) {
            check(allowed()) { "Android 主核生命周期已变化" }
            val next = (mainClaimRevisions[owner] ?: 0L) + 1L
            mainClaimRevisions[owner] = next
            mainClaims[owner] = mainClaims[owner].orEmpty() + directories
            val conflicts = synchronized(entries) { entries.values.filter { it.stateDirectories.any(directories::contains) } }
            for (entry in conflicts) {
                entry.cancelled = true
                check(dispose(entry) == null) { "Android 旧登录实例尚未关闭" }
                synchronized(entries) { if (entries[entry.id] === entry) entries.remove(entry.id) }
            }
            next
        }
        // The claim is visible to login instances. Stop revokes immediately; its
        // close worker joins operationLock before closing this Start/Reload.
        val result = action()
        synchronized(ownershipLock) {
            check(allowed() && mainClaims.containsKey(owner)) { "Android 主核生命周期已变化" }
            if (mainClaimRevisions[owner] == revision) mainClaims[owner] = directories
        }
        result
    }

    fun closeMain(owner: Any, action: () -> Unit) {
        action()
        synchronized(ownershipLock) {
            mainClaims.remove(owner)
            mainClaimRevisions.remove(owner)
        }
    }

    fun start(id: String, config: String, done: (StartFailure?) -> Unit) {
        val directories = try {
            SystemEndpointGuard.requireSupported(config)
            stateDirectories(config)
        } catch (error: Exception) {
            done(if (error is SystemEndpointGuard.Unsupported) {
                SystemInterfaceFailure()
            } else {
                GeneralFailure("Android 独立登录失败 [config/INVALID]")
            })
            return
        }
        val entry = synchronized(entries) {
            if (id.isBlank() || id.length > 256 || entries.containsKey(id) || entries.size >= MAX_INSTANCES) {
                done(GeneralFailure("Android 登录实例标识重复或并发上限已到")); return
            }
            Entry(id, directories).also { entries[id] = it }
        }
        try {
            AndroidNativeValidation.enqueue({ worker.execute(it) }) { validationTicket ->
                var stage = "ownership"
                val failure = synchronized(ownershipLock) { runCatching {
                    check(!entry.cancelled) { "Android 登录请求已取消" }
                    check(mainClaims.values.none { claim -> entry.stateDirectories.any(claim::contains) }) { "Android Tailscale 端点已被主核持有" }
                    val predecessors = synchronized(entries) {
                        entries.values.filter { it !== entry && !it.disposed && it.stateDirectories.any(entry.stateDirectories::contains) }
                    }
                    for (previous in predecessors) {
                        previous.cancelled = true
                        check(dispose(previous) == null) { "Android 旧登录实例尚未关闭" }
                        synchronized(entries) { if (entries[previous.id] === previous) entries.remove(previous.id) }
                    }
                    stage = "setup"
                    PolarisApplication.ensureSetup()
                    check(!Libbox.hasTunInbound(config)) { "Android 独立登录不允许创建 VPN 隧道" }
                    stage = "check"
                    AndroidNativeValidation.check(validationTicket, config)
                    stage = "cache"
                    val cachePath = JSONObject(config).optJSONObject("experimental")?.optJSONObject("cache_file")?.optString("path").orEmpty()
                    val cache = File(cachePath).canonicalFile
                    check(cache.parent in entry.stateDirectories && cache.name.matches(Regex("login-cache-[0-9]+\\.db"))) { "Android 登录缓存路径未隔离" }
                    // Cache Initialize precedes endpoint Initialize; a first login has no TS directory yet.
                    ensurePrivateDirectory(requireNotNull(cache.parentFile))
                    entry.cache = cache
                    val network = TransientLoginNetwork()
                    entry.network = network
                    stage = "network"
                    network.start()
                    check(!entry.cancelled) { "Android 登录请求已取消" }
                    // The factory disables global command socket/snapshot/power reports inside libbox.
                    stage = "factory"
                    val server = Libbox.newTransientCommandServer(LoginHandler(entry), network)
                    entry.server = server
                    stage = "start"
                    server.startOrReloadService(config, OverrideOptions())
                    check(!entry.cancelled) { "Android 登录请求已取消" }
                    entry.running = true
                    // Rust owns normal cancellation/Running/timeout. This bounds a detached late bridge invocation too.
                    timer.schedule({ close(id) {} }, 300, TimeUnit.SECONDS)
                }.exceptionOrNull() }
                // A rejected/cancelled worker that never reached validation is terminal before birth.
                AndroidNativeValidation.cancelBeforeBirth(validationTicket)
                if (failure != null) {
                    val cleanup = synchronized(ownershipLock) { dispose(entry) }
                    if (cleanup == null) synchronized(entries) { if (entries[id] === entry) entries.remove(id) }
                    if (cleanup != null) timer.schedule({ close(id) {} }, 5, TimeUnit.SECONDS)
                    val message = failure.message.orEmpty().lowercase()
                    val reason = when {
                        "no such file" in message -> "PATH_NOT_FOUND"
                        "permission" in message || "denied" in message -> "PERMISSION_DENIED"
                        "unknown" in message -> "UNSUPPORTED"
                        "timeout" in message -> "TIMEOUT"
                        "可用的物理网络" in message -> "NO_PHYSICAL_NETWORK"
                        else -> "FAILED"
                    }
                    Log.w("PolarisLogin", "transient failure stage=$stage type=${failure.javaClass.simpleName} reason=$reason")
                    done(GeneralFailure("Android 独立登录失败 [$stage/$reason]"))
                } else done(null)
            }
        } catch (_: Throwable) {
            synchronized(entries) { if (entries[id] === entry) entries.remove(id) }
            done(GeneralFailure("Android 独立登录失败 [admission/UNAVAILABLE]"))
        }
    }

    fun close(id: String, done: (String?) -> Unit) {
        val entry = synchronized(entries) { entries[id]?.also { it.cancelled = true } }
        if (entry == null) { done(null); return }
        worker.execute {
            val failure = synchronized(ownershipLock) { dispose(entry) }
            if (failure == null) synchronized(entries) { if (entries[id] === entry) entries.remove(id) }
            if (failure != null) timer.schedule({ close(id) {} }, 5, TimeUnit.SECONDS)
            done(failure)
        }
    }

    fun running(id: String): Boolean = synchronized(entries) { entries[id]?.running == true }

    /** Close acknowledgements are sent only after this instance's Go service and network callback are gone. */
    private fun dispose(entry: Entry): String? {
        if (entry.disposed) return null
        var failure: Throwable? = null
        entry.server?.let { server ->
            runCatching { server.closeService() }.onFailure { failure = it }
            runCatching { server.close() }.onFailure { if (failure == null) failure = it }
            if (failure == null) entry.server = null
        }
        runCatching { entry.network?.close() }.onFailure { if (failure == null) failure = it }
        if (failure == null) entry.network = null
        if (failure == null) {
            runCatching { entry.cache?.let { check(!it.exists() || it.delete()) { "Android 登录缓存清理失败" } } }
                .onFailure { failure = it }
            if (failure == null) {
                entry.running = false
                entry.disposed = true
                entry.cache = null
            }
        }
        return failure?.let { it.message ?: "Android 登录实例关闭失败" }
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

    private class LoginHandler(private val entry: Entry) : CommandServerHandler {
        override fun serviceStop() { close(entry.id) {} }
        override fun serviceReload() { error("Android 独立登录实例不能重载主代理") }
        override fun getSystemProxyStatus() = SystemProxyStatus().apply { available = false; enabled = false }
        override fun setSystemProxyEnabled(enabled: Boolean) { error("Android 独立登录不允许设置系统代理") }
        override fun triggerNativeCrash() { error("Android 独立登录不提供崩溃指令") }
        override fun writeDebugMessage(message: String?) { /* Auth URLs/secrets stay in the STATUS stream. */ }
        override fun connectSSHAgent(): Int = -1
    }
}
