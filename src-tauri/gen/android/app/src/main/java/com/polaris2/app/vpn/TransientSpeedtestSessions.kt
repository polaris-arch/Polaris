package com.polaris2.app.vpn

import java.util.concurrent.CompletableFuture
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.ScheduledThreadPoolExecutor
import java.util.concurrent.ThreadFactory
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/** Native ownership ledger. All waits and Engine calls happen outside [lock]. */
internal class TransientSpeedtestSessions(
    private val closeTimeoutMillis: Long = 8_000,
    private val expiryMillis: Long = 300_000,
    private val logFailure: (Throwable) -> Unit = {},
    private val nativeAdmission: AndroidNativeAdmission? = null,
    private val launchStart: (() -> Unit) -> Unit = { Thread(it, "polaris-speedtest-start").start() },
) {
    companion object {
        // Owner tickets are installed, but queried DNS callbacks still lack an
        // exact drain contract. Keep this family out of the coverage manifest.
        val capabilities = emptySet<AndroidNativeProducer>()
    }
    interface Engine {
        fun prepare()
        fun start()
        /** Existing operational Close result; supplemental cleanup proof is separate. */
        fun close()
        /** Supplemental proof only; false must not turn a successful operational Close into failure. */
        fun cleanupConfirmed(): Boolean = true
    }

    private class Entry(val id: String, val sequence: String, val engine: Engine,
        val nativeTicket: AndroidNativeAdmission.Ticket?) {
        var revoked = false
        var prepared = false
        var closeLaunched = false
        var expiry: ScheduledFuture<*>? = null
        var state = "starting"
        val closed = CompletableFuture<Unit>()
        val operationFinished = CompletableFuture<Unit>()
        var constructionUnknown = false
    }

    private data class Identity(val epoch: String, val sequence: String)
    private val idPattern = Regex("([0-9a-f]{32}):([0-9a-f]{16})")
    private val zeroSequence = "0000000000000000"
    private val lock = Object()
    private var epoch: String? = null
    // Fixed-width hex makes lexical and unsigned numeric order identical. A close
    // revokes every earlier sequence, so no per-session tombstones or TTL are needed.
    private var closedThrough = zeroSequence
    private val mainOwners = mutableSetOf<Any>()
    private var active: Entry? = null
    private val timer = ScheduledThreadPoolExecutor(1, ThreadFactory { runnable ->
        Thread(runnable, "polaris-speedtest-timer").apply { isDaemon = true }
    }).apply { removeOnCancelPolicy = true }

    /** Host calls this before its config-validation thread or session worker is queued. */
    fun reserveOwner(id: String): AndroidNativeAdmission.Ticket {
        require(parseIdentity(id) != null) { "Android 临时测速实例标识无效" }
        return checkNotNull(nativeAdmission).reserveOwner(AndroidNativeAdmission.Kind.Speedtest, id)
    }

    fun cancelBeforeBirth(ticket: AndroidNativeAdmission.Ticket) {
        require(ticket.kind == AndroidNativeAdmission.Kind.Speedtest)
        nativeAdmission?.cancelBeforeBirth(ticket)
    }

    fun start(id: String, engine: Engine, nativeTicket: AndroidNativeAdmission.Ticket? = null, done: (String?) -> Unit) =
        startCoded(id, engine, nativeTicket) { done(it?.message) }

    fun startCoded(id: String, engine: Engine, nativeTicket: AndroidNativeAdmission.Ticket? = null, done: (AndroidNativeFailure?) -> Unit) {
        val identity = parseIdentity(id)
        // A rejected invocation has no custody of a different family or ID.
        val ownerTicket = nativeTicket?.takeIf {
            it.kind == AndroidNativeAdmission.Kind.Speedtest && it.logicalId == id
        }
        if (identity == null || (nativeAdmission != null && ownerTicket == null)) {
            ownerTicket?.let(::cancelBeforeBirth)
            done(AndroidNativeFailure("Android 临时测速实例标识无效"))
            return
        }
        val entry = synchronized(lock) {
            if (epoch == null) epoch = identity.epoch
            if (epoch != identity.epoch || identity.sequence <= closedThrough) null
            else {
                // Consuming before admission means a busy/rejected invocation cannot
                // be replayed after the current owner eventually closes.
                rememberClosed(identity.sequence)
                if (mainOwners.isNotEmpty() || active != null) null
                else Entry(id, identity.sequence, engine, ownerTicket).also { active = it }
            }
        }
        if (entry == null) {
            ownerTicket?.let(::cancelBeforeBirth)
            done(AndroidNativeFailure("Android 临时测速实例忙或标识重复"))
            return
        }
        try { launchStart {
            var failure: AndroidNativeFailure? = null
            try {
                if (!isRevoked(entry)) {
                    if (nativeAdmission != null && !nativeAdmission.enterBirth(checkNotNull(ownerTicket))) {
                        cancelBeforeBirth(ownerTicket)
                        throw nativeAdmission.admissionRejection()
                    }
                    entry.engine.prepare()
                    synchronized(lock) { entry.prepared = true; lock.notifyAll() }
                    if (!isRevoked(entry)) {
                        try {
                            entry.engine.start()
                            synchronized(lock) {
                                if (entry.revoked) markConstructionUnknown(entry)
                            }
                        } catch (error: Throwable) {
                            synchronized(lock) { markConstructionUnknown(entry) }
                            throw error
                        }
                    }
                    if (isRevoked(entry)) failure = AndroidNativeFailure("Android 临时测速已取消")
                    else synchronized(lock) {
                        if (!entry.revoked && active === entry) entry.state = "running"
                        else failure = AndroidNativeFailure("Android 临时测速已取消")
                    }
                } else failure = AndroidNativeFailure("Android 临时测速已取消")
            } catch (error: Throwable) {
                synchronized(lock) {
                    if (ownerTicket != null && nativeAdmission?.state(ownerTicket) == AndroidNativeAdmission.State.BirthEntered) {
                        markConstructionUnknown(entry)
                    }
                }
                logFailure(error)
                failure = AndroidNativeFailure.from(error, "Android 临时测速启动失败")
            } finally {
                synchronized(lock) { entry.prepared = true; lock.notifyAll() }
                entry.operationFinished.complete(Unit)
            }
            if (failure != null) requestClose(entry)
            else synchronized(lock) {
                if (active === entry && !entry.revoked) {
                    entry.expiry = timer.schedule({ requestClose(entry) }, expiryMillis, TimeUnit.MILLISECONDS)
                }
            }
            done(failure)
        } } catch (error: Throwable) {
            ownerTicket?.let(::cancelBeforeBirth)
            synchronized(lock) { entry.prepared = true; lock.notifyAll() }
            entry.operationFinished.complete(Unit)
            requestClose(entry)
            logFailure(error)
            done(AndroidNativeFailure("Android 临时测速启动线程不可用"))
        }
    }

    fun close(id: String, done: (String?) -> Unit) = closeCoded(id) { done(it?.message) }

    fun closeCoded(id: String, done: (AndroidNativeFailure?) -> Unit) {
        val identity = parseIdentity(id)
        if (identity == null) {
            done(AndroidNativeFailure("Android 临时测速实例标识无效"))
            return
        }
        val (entry, wrongEpoch) = try { synchronized(lock) {
            if (epoch == null) epoch = identity.epoch
            if (epoch != identity.epoch) Pair(null, true)
            else {
                nativeAdmission?.retireOwner(AndroidNativeAdmission.Kind.Speedtest, id)
                val owned = active?.takeIf { it.id == id }
                // Close may overtake start. Advance the watermark before acknowledging;
                // a delayed start for this or any older generation is then impossible.
                if (owned == null) rememberClosed(identity.sequence)
                Pair(owned, false)
            }
        }
        } catch (error: AndroidNativeAdmission.CapacityClosed) {
            done(AndroidNativeFailure.from(error, "Android 临时测速原生准入已关闭"))
            return
        }
        if (wrongEpoch) { done(AndroidNativeFailure("Android 临时测速实例 epoch 不匹配")); return }
        if (entry == null) { done(null); return }
        requestClose(entry)
        val replied = AtomicBoolean(false)
        val deadline = timer.schedule({
            synchronized(lock) {
                if (active === entry && !entry.closed.isDone) entry.state = "cleanupUnknown"
            }
            if (replied.compareAndSet(false, true)) done(AndroidNativeFailure("Android 临时测速关闭结果未知"))
        }, closeTimeoutMillis, TimeUnit.MILLISECONDS)
        entry.closed.whenComplete { _, error ->
            deadline.cancel(false)
            if (replied.compareAndSet(false, true)) {
                done(if (error == null) null else AndroidNativeFailure("Android 临时测速关闭失败，清理结果未知"))
            }
        }
    }

    fun status(id: String): String {
        val identity = parseIdentity(id) ?: return "unknown"
        return synchronized(lock) {
            when {
                active?.id == id -> active!!.state
                epoch == identity.epoch && identity.sequence <= closedThrough -> "closed"
                else -> "unknown"
            }
        }
    }

    /** Claim first, revoke token second, wait boundedly, then enter main JNI. */
    fun <T> withMainStart(owner: Any, allowed: () -> Boolean, action: () -> T): T {
        val (previous, createdClaim) = synchronized(lock) {
            check(allowed()) { "Android 主核生命周期已变化" }
            check(mainOwners.none { it !== owner }) { "Android 旧主核实例尚未确认关闭" }
            val created = mainOwners.add(owner)
            Pair(active?.also { it.revoked = true; it.state = "closing" }, created)
        }
        if (previous != null) {
            requestClose(previous)
            try {
                previous.closed.get(closeTimeoutMillis, TimeUnit.MILLISECONDS)
            } catch (_: Exception) {
                synchronized(lock) {
                    if (active === previous) previous.state = "cleanupUnknown"
                    if (createdClaim) mainOwners.remove(owner)
                }
                throw IllegalStateException("Android 临时测速 cleanupUnknown：主核启动已拒绝")
            }
        }
        if (!allowed()) {
            synchronized(lock) { if (createdClaim) mainOwners.remove(owner) }
            error("Android 主核生命周期已变化")
        }
        // If action constructs a native main server and fails, BoxService.closeMain retains
        // this claim until its own confirmed close succeeds.
        return action()
    }

    fun closeMain(owner: Any, action: () -> Unit) {
        action()
        synchronized(lock) { mainOwners.remove(owner) }
    }

    private fun parseIdentity(id: String): Identity? {
        val match = idPattern.matchEntire(id) ?: return null
        val sequence = match.groupValues[2]
        if (sequence == zeroSequence) return null
        return Identity(match.groupValues[1], sequence)
    }

    /** Caller holds [lock]. */
    private fun rememberClosed(sequence: String) {
        if (sequence > closedThrough) closedThrough = sequence
    }

    private fun isRevoked(entry: Entry): Boolean = synchronized(lock) { entry.revoked || active !== entry }

    /** Caller holds lock; completion is published only after this sticky fact. */
    private fun markConstructionUnknown(entry: Entry) {
        if (nativeAdmission == null) return
        entry.constructionUnknown = true
        entry.nativeTicket?.let(nativeAdmission::unknown)
    }

    private fun requestClose(entry: Entry) {
        val launch = synchronized(lock) {
            if (active !== entry || entry.closeLaunched) false
            else {
                entry.revoked = true
                entry.state = "closing"
                entry.closeLaunched = true
                entry.expiry?.cancel(false)
                entry.expiry = null
                true
            }
        }
        if (!launch) return
        Thread({
            synchronized(lock) { while (!entry.prepared) lock.wait() }
            // Close is per entry, so a blocked JNI Start never queues this behind itself.
            var failure = runCatching { entry.engine.close() }.exceptionOrNull()
            // Close may cancel a blocked native Start, but cannot settle before
            // that exact worker passes its final possible native acquisition.
            entry.operationFinished.get()
            val cleanupConfirmed = runCatching { entry.engine.cleanupConfirmed() }.getOrDefault(false)
            synchronized(lock) {
                if (failure == null && entry.nativeTicket != null && nativeAdmission != null) {
                    val ticket = entry.nativeTicket
                    val state = nativeAdmission.state(ticket)
                    val settled = if (entry.constructionUnknown || !cleanupConfirmed) false else when (state) {
                        AndroidNativeAdmission.State.Reserved -> nativeAdmission.cancelBeforeBirth(ticket)
                        AndroidNativeAdmission.State.CancelledBeforeBirth -> true
                        AndroidNativeAdmission.State.BirthEntered -> nativeAdmission.closedExact(ticket)
                        else -> false
                    }
                    if (!settled) nativeAdmission.unknown(ticket)
                }
                if (failure != null) entry.nativeTicket?.let { nativeAdmission?.unknown(it) }
            }
            synchronized(lock) {
                if (failure == null) {
                    if (active === entry) active = null
                    rememberClosed(entry.sequence)
                    entry.state = "closed"
                } else {
                    entry.state = "cleanupUnknown"
                }
            }
            if (failure == null) entry.closed.complete(Unit)
            else {
                val terminalFailure = checkNotNull(failure)
                logFailure(terminalFailure)
                entry.closed.completeExceptionally(terminalFailure)
            }
        }, "polaris-speedtest-close").start()
    }
}
