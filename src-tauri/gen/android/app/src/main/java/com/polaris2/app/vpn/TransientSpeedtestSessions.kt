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
) {
    interface Engine {
        fun prepare()
        fun start()
        /** Return only after native CloseService and all ancillary resources are closed. */
        fun close()
    }

    private class Entry(val id: String, val sequence: String, val engine: Engine) {
        var revoked = false
        var prepared = false
        var closeLaunched = false
        var expiry: ScheduledFuture<*>? = null
        var state = "starting"
        val closed = CompletableFuture<Unit>()
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
    }.apply { removeOnCancelPolicy = true }

    fun start(id: String, engine: Engine, done: (String?) -> Unit) {
        val identity = parseIdentity(id)
        if (identity == null) {
            done("Android 临时测速实例标识无效")
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
                else Entry(id, identity.sequence, engine).also { active = it }
            }
        }
        if (entry == null) {
            done("Android 临时测速实例忙或标识重复")
            return
        }
        Thread({
            var failure: String? = null
            try {
                if (!isRevoked(entry)) {
                    entry.engine.prepare()
                    synchronized(lock) { entry.prepared = true; lock.notifyAll() }
                    if (!isRevoked(entry)) entry.engine.start()
                    if (isRevoked(entry)) failure = "Android 临时测速已取消"
                    else synchronized(lock) {
                        if (!entry.revoked && active === entry) entry.state = "running"
                        else failure = "Android 临时测速已取消"
                    }
                } else failure = "Android 临时测速已取消"
            } catch (error: Throwable) {
                logFailure(error)
                failure = "Android 临时测速启动失败"
            } finally {
                synchronized(lock) { entry.prepared = true; lock.notifyAll() }
            }
            if (failure != null) requestClose(entry)
            else synchronized(lock) {
                if (active === entry && !entry.revoked) {
                    entry.expiry = timer.schedule({ requestClose(entry) }, expiryMillis, TimeUnit.MILLISECONDS)
                }
            }
            done(failure)
        }, "polaris-speedtest-start").start()
    }

    fun close(id: String, done: (String?) -> Unit) {
        val identity = parseIdentity(id)
        if (identity == null) {
            done("Android 临时测速实例标识无效")
            return
        }
        val (entry, wrongEpoch) = synchronized(lock) {
            if (epoch == null) epoch = identity.epoch
            if (epoch != identity.epoch) Pair(null, true)
            else {
                val owned = active?.takeIf { it.id == id }
                // Close may overtake start. Advance the watermark before acknowledging;
                // a delayed start for this or any older generation is then impossible.
                if (owned == null) rememberClosed(identity.sequence)
                Pair(owned, false)
            }
        }
        if (wrongEpoch) { done("Android 临时测速实例 epoch 不匹配"); return }
        if (entry == null) { done(null); return }
        requestClose(entry)
        val replied = AtomicBoolean(false)
        val deadline = timer.schedule({
            synchronized(lock) {
                if (active === entry && !entry.closed.isDone) entry.state = "cleanupUnknown"
            }
            if (replied.compareAndSet(false, true)) done("Android 临时测速关闭结果未知")
        }, closeTimeoutMillis, TimeUnit.MILLISECONDS)
        entry.closed.whenComplete { _, error ->
            deadline.cancel(false)
            if (replied.compareAndSet(false, true)) {
                done(if (error == null) null else "Android 临时测速关闭失败，清理结果未知")
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
            val failure = runCatching { entry.engine.close() }.exceptionOrNull()
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
                logFailure(failure)
                entry.closed.completeExceptionally(failure)
            }
        }, "polaris-speedtest-close").start()
    }
}
