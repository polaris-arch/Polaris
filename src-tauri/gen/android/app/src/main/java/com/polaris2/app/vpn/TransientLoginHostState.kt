package com.polaris2.app.vpn

import java.util.concurrent.atomic.AtomicBoolean

/** Shared production close stages. No lock or supplemental proof wait is added here. */
internal class TransientHostCleanup(
    private val beginResolverClose: () -> Unit,
    private val closeService: () -> Unit,
    private val closeServer: () -> Unit,
    private val closeNetwork: () -> Unit,
    private val nativeClosed: () -> Unit = {},
    private val networkClosed: () -> Unit = {},
    private val closeCache: () -> Unit = {},
    private val resolverUnknown: () -> Unit = {},
) {
    fun close() {
        // Fence first; do not let a fence failure hide a real native/network cleanup failure.
        val fenceFailure = runCatching { beginResolverClose() }.exceptionOrNull()
        var failure: Throwable? = null
        runCatching { closeService() }.onFailure { failure = it }
        runCatching { closeServer() }.onFailure { if (failure == null) failure = it }
        if (failure == null && fenceFailure == null) nativeClosed()
        runCatching { closeNetwork() }.onFailure {
            if (it is TransientResolverLifecycle.CleanupUnknown) resolverUnknown()
            else if (failure == null) failure = it
        }
        if (failure == null && fenceFailure == null) {
            networkClosed()
            runCatching { closeCache() }.onFailure { failure = it }
        }
        (failure ?: fenceFailure)?.let { throw it }
    }
}

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
        fun bindTailscaleStore(store: AndroidTailscaleStoreCustody) {}
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
            parseDirectories(config).also { if (ledger.isWarmLogin(nativeOwner.ticket)) ledger.bindWarmConfig(nativeOwner.ticket, config) }
        } catch (error: Throwable) {
            ledger.finishWarmPreparation(nativeOwner.ticket)
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
            ledger.finishWarmPreparation(nativeOwner.ticket)
            nativeOwner.cancel()
            reply(TransientLoginHost.GeneralFailure("Android 登录实例标识重复或并发上限已到"))
            return
        }
        try {
            val enqueue: ((AndroidNativeAdmission.Ticket) -> Unit) -> Unit = { action ->
                if (ledger.isWarmLogin(nativeOwner.ticket)) AndroidNativeValidation.enqueueWarm(ledger, nativeOwner.ticket, { queue(it) }, action)
                else nativeOwner.enqueue({ queue(it) }) { validationTicket -> action(validationTicket) }
            }
            enqueue { validationTicket ->
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
                        engine.bindTailscaleStore(entry.nativeOwner.bindTailscaleStore())
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
            ledger.finishWarmPreparation(nativeOwner.ticket)
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

    /** Select once by full original binding; completion cannot look up a successor by ID. */
    fun closeTailscale(binding: org.json.JSONObject, done: (AndroidNativeFailure?) -> Unit) {
        val warm = binding.optString("contractVersion") == "polaris-android-ts-cold-warm-v1"
        val warmTicket = if (warm) runCatching { ledger.warmTicket(binding) }.getOrNull() else null
        // A claimed Start may still be parsing before its Host.Entry exists.
        // Cancel that exact reserved ticket; a born factory stays Unknown and closes below.
        warmTicket?.let { ledger.cancelBeforeBirth(it) }
        val original = if (warm) null else ledger.readTailscaleOwner(binding)
        val entry = synchronized(entries) { entries[binding.optString("logicalInstanceId")]?.takeIf {
            it.nativeOwner.ticket.id == (warmTicket?.id ?: binding.optString("nativeTicketId"))
        } }
        if ((warm && warmTicket == null) || (!warm && original == null)) { done(AndroidNativeFailure("nativeRetirementUnknown")); return }
        if (entry == null) { done(null); return } // Only the retained original Entry supplies the later receipt.
        close(entry) { done(it?.let(::AndroidNativeFailure)) }
    }

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
