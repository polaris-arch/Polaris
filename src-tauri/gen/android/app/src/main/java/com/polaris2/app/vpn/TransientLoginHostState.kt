package com.polaris2.app.vpn

import java.util.concurrent.atomic.AtomicBoolean

/** Production login lifecycle; JNI, Android networking, queue and clock are injected boundaries. */
internal class TransientLoginHostState(
    private val ledger: AndroidNativeAdmission,
    private val queue: (() -> Unit) -> Unit,
    private val schedule: (Long, () -> Unit) -> Unit,
    private val parseDirectories: (String) -> Set<String>,
    private val requireSupported: (String) -> Unit,
    private val createEngine: (String, String, Set<String>, () -> Unit) -> Engine,
    private val logFailure: (String, Throwable, String) -> Unit = { _, _, _ -> },
    private val maxInstances: Int = 8,
) {
    interface Engine {
        fun prepare(validationTicket: AndroidNativeAdmission.Ticket, stage: (String) -> Unit, cancelled: () -> Boolean)
        fun start()
        /** Same operational native/network/cache cleanup as the production adapter. */
        fun close()
    }
    private class Entry(val id: String, val stateDirectories: Set<String>, val nativeOwner: TransientLoginNativeOwner) {
        val cancelled: Boolean get() = nativeOwner.cancelled
        @Volatile var running = false
        var engine: Engine? = null
        var disposed = false
    }
    private val entries = mutableMapOf<String, Entry>()
    private val ownershipLock = Any()
    private val mainClaims = mutableMapOf<Any, Set<String>>()
    private val mainClaimRevisions = mutableMapOf<Any, Long>()

    /** Claims precede core startup and survive failures until its actual close succeeds. */
    fun <T> withMainConfig(owner: MainKernelAttempt<*>, config: String, allowed: () -> Boolean = { true }, action: () -> T): T = synchronized(owner.operationLock) {
        val directories = parseDirectories(config)
        val revision = synchronized(ownershipLock) {
            check(allowed()) { "Android 主核生命周期已变化" }
            val next = (mainClaimRevisions[owner] ?: 0L) + 1L
            mainClaimRevisions[owner] = next
            mainClaims[owner] = mainClaims[owner].orEmpty() + directories
            val conflicts = synchronized(entries) { entries.values.filter { it.stateDirectories.any(directories::contains) } }
            for (entry in conflicts) {
                entry.nativeOwner.cancel()
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

    fun start(id: String, config: String, done: (TransientLoginHost.StartFailure?) -> Unit) {
        val reply = completion(done)
        val nativeOwner = try {
            TransientLoginNativeOwner.reserve(ledger, id)
        } catch (error: Throwable) {
            reply(startFailure(error, "Android 独立登录失败 [admission/UNAVAILABLE]"))
            return
        }
        val directories = try {
            requireSupported(config)
            parseDirectories(config)
        } catch (error: Throwable) {
            nativeOwner.cancel()
            reply(if (error is SystemEndpointGuard.Unsupported) {
                TransientLoginHost.SystemInterfaceFailure()
            } else {
                TransientLoginHost.GeneralFailure("Android 独立登录失败 [config/INVALID]")
            })
            return
        }
        val entry = synchronized(entries) {
            if (nativeOwner.cancelled || entries.containsKey(id) || entries.size >= maxInstances) {
                null
            } else Entry(id, directories, nativeOwner).also { entries[id] = it }
        }
        if (entry == null) {
            nativeOwner.cancel()
            reply(TransientLoginHost.GeneralFailure("Android 登录实例标识重复或并发上限已到"))
            return
        }
        try {
            nativeOwner.enqueue({ queue(it) }) { validationTicket ->
                var stage = "ownership"
                val failure = synchronized(ownershipLock) { runCatching {
                    if (!ledger.birthAllowed(entry.nativeOwner.ticket)) throw ledger.admissionRejection()
                    check(!entry.cancelled) { "Android 登录请求已取消" }
                    check(mainClaims.values.none { claim -> entry.stateDirectories.any(claim::contains) }) { "Android Tailscale 端点已被主核持有" }
                    val predecessors = synchronized(entries) {
                        entries.values.filter { it !== entry && !it.disposed && it.stateDirectories.any(entry.stateDirectories::contains) }
                    }
                    for (previous in predecessors) {
                        previous.nativeOwner.cancel()
                        check(dispose(previous) == null) { "Android 旧登录实例尚未关闭" }
                        synchronized(entries) { if (entries[previous.id] === previous) entries.remove(previous.id) }
                    }
                    entry.nativeOwner.construct {
                        val engine = createEngine(id, config, entry.stateDirectories) { close(entry) {} }
                        entry.engine = engine
                        engine.prepare(validationTicket, { stage = it }, { entry.cancelled })
                        stage = "start"
                        engine.start()
                        check(!entry.cancelled) { "Android 登录请求已取消" }
                        entry.running = true
                        // Rust owns normal cancellation/Running/timeout. This bounds a detached late bridge invocation too.
                        schedule(300_000) { close(entry) {} }
                    }
                }.exceptionOrNull() }
                // A rejected/cancelled worker that never reached validation is terminal before birth.
                ledger.cancelBeforeBirth(validationTicket)
                if (failure != null) {
                    val cleanup = synchronized(ownershipLock) { dispose(entry) }
                    if (cleanup == null) synchronized(entries) { if (entries[id] === entry) entries.remove(id) }
                    if (cleanup != null) retry(entry)
                    val message = failure.message.orEmpty().lowercase()
                    val reason = when {
                        "no such file" in message -> "PATH_NOT_FOUND"
                        "permission" in message || "denied" in message -> "PERMISSION_DENIED"
                        "unknown" in message -> "UNSUPPORTED"
                        "timeout" in message -> "TIMEOUT"
                        "可用的物理网络" in message -> "NO_PHYSICAL_NETWORK"
                        else -> "FAILED"
                    }
                    report(stage, failure, reason)
                    reply(startFailure(failure, "Android 独立登录失败 [$stage/$reason]"))
                } else reply(null)
            }
        } catch (error: Throwable) {
            nativeOwner.cancel()
            synchronized(entries) { if (entries[id] === entry) entries.remove(id) }
            reply(startFailure(error, "Android 独立登录失败 [admission/UNAVAILABLE]"))
        }
    }

    fun close(id: String, done: (String?) -> Unit) {
        closeCoded(id) { done(it?.message) }
    }

    fun closeCoded(id: String, done: (AndroidNativeFailure?) -> Unit) {
        val reply = completion(done)
        val entry = try { synchronized(entries) {
            TransientLoginNativeOwner.retireBeforeStart(ledger, id)
            entries[id]?.also { it.nativeOwner.cancel() }
        } } catch (_: IllegalArgumentException) {
            reply(AndroidNativeFailure("Android 登录实例标识无效"))
            return
        } catch (error: AndroidNativeAdmission.CapacityClosed) {
            reply(AndroidNativeFailure.from(error, "Android 登录原生准入已关闭"))
            return
        }
        if (entry == null) { reply(null); return }
        close(entry) { reply(it?.let(::AndroidNativeFailure)) }
    }

    private fun startFailure(error: Throwable, fallback: String): TransientLoginHost.GeneralFailure {
        val failure = AndroidNativeFailure.from(error, fallback)
        return TransientLoginHost.GeneralFailure(failure.message, failure.code)
    }

    /** Timers, retries and native callbacks retain this exact entry, never a later ID lookup. */
    private fun close(entry: Entry, done: (String?) -> Unit) {
        val reply = completion(done)
        entry.nativeOwner.cancel()
        try { queue {
            val failure = synchronized(ownershipLock) { dispose(entry) }
            if (failure == null) synchronized(entries) { if (entries[entry.id] === entry) entries.remove(entry.id) }
            if (failure != null) retry(entry)
            reply(failure)
        } } catch (_: Throwable) {
            entry.nativeOwner.constructionFailed()
            reply("Android 登录实例关闭未确认")
        }
    }

    fun running(id: String): Boolean = synchronized(entries) { entries[id]?.running == true }

    /** Close acknowledgements are sent only after this instance's Go service and network callback are gone. */
    private fun dispose(entry: Entry): String? {
        if (entry.disposed) return null
        val failure = runCatching { entry.engine?.close() }.exceptionOrNull()
        if (failure == null) {
            entry.engine = null
            entry.running = false
            entry.disposed = true
            entry.nativeOwner.closedWithoutProof()
        } else entry.nativeOwner.constructionFailed()
        return failure?.let { it.message ?: "Android 登录实例关闭失败" }
    }

    private fun retry(entry: Entry) {
        try { schedule(5_000) { close(entry) {} } }
        catch (failure: Throwable) {
            entry.nativeOwner.constructionFailed()
            report("retry", failure, "FAILED")
        }
    }

    private fun report(stage: String, failure: Throwable, reason: String) {
        runCatching { logFailure(stage, failure, reason) }
    }

    private fun <T> completion(done: (T) -> Unit): (T) -> Unit {
        val once = AtomicBoolean(false)
        return { result ->
            if (once.compareAndSet(false, true)) {
                try { done(result) } catch (failure: Throwable) { report("callback", failure, "FAILED") }
            }
        }
    }
}
