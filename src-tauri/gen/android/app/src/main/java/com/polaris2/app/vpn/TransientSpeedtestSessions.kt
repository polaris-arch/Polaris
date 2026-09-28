package com.polaris2.app.vpn

import java.util.concurrent.CompletableFuture
import java.util.concurrent.Executors
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

    private class Entry(val id: String, val engine: Engine) {
        var revoked = false
        var prepared = false
        var closeLaunched = false
        var state = "starting"
        val closed = CompletableFuture<Unit>()
    }

    private val lock = Object()
    private val closedIds = LinkedHashSet<String>()
    private val mainOwners = mutableSetOf<Any>()
    private var active: Entry? = null
    // Never evict a revoked ID: a delayed IPC start could otherwise revive it.
    // Exhaustion closes speedtest admission until process restart.
    private var admissionClosed = false
    private val timer = Executors.newSingleThreadScheduledExecutor {
        Thread(it, "polaris-speedtest-timer").apply { isDaemon = true }
    }

    fun start(id: String, engine: Engine, done: (String?) -> Unit) {
        if (id.isBlank() || id.length > 256) {
            done("Android 临时测速实例标识无效")
            return
        }
        val entry = synchronized(lock) {
            if (admissionClosed || mainOwners.isNotEmpty() || active != null || id in closedIds) null
            else Entry(id, engine).also { active = it }
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
            else timer.schedule({ requestClose(entry) }, expiryMillis, TimeUnit.MILLISECONDS)
            done(failure)
        }, "polaris-speedtest-start").start()
    }

    fun close(id: String, done: (String?) -> Unit) {
        if (id.isBlank() || id.length > 256) {
            done("Android 临时测速实例标识无效")
            return
        }
        val entry = synchronized(lock) {
            active?.takeIf { it.id == id } ?: run {
                // Close may overtake the start IPC. This cancellation tombstone prevents
                // a late start from constructing native resources after close resolves.
                rememberClosed(id)
                null
            }
        }
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

    fun status(id: String): String = synchronized(lock) {
        if (active?.id == id) active!!.state
        else if (id in closedIds) "closed" else "unknown"
    }

    /** Claim first, revoke token second, wait boundedly, then enter main JNI. */
    fun <T> withMainStart(owner: Any, allowed: () -> Boolean, action: () -> T): T {
        val previous = synchronized(lock) {
            check(allowed()) { "Android 主核生命周期已变化" }
            mainOwners.add(owner)
            active?.also { it.revoked = true; it.state = "closing" }
        }
        if (previous != null) {
            requestClose(previous)
            try {
                previous.closed.get(closeTimeoutMillis, TimeUnit.MILLISECONDS)
            } catch (_: Exception) {
                synchronized(lock) {
                    if (active === previous) previous.state = "cleanupUnknown"
                    mainOwners.remove(owner)
                }
                throw IllegalStateException("Android 临时测速 cleanupUnknown：主核启动已拒绝")
            }
        }
        if (!allowed()) {
            synchronized(lock) { mainOwners.remove(owner) }
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

    /** Caller holds [lock]. */
    private fun rememberClosed(id: String) {
        closedIds.add(id)
        if (closedIds.size >= 2_048) admissionClosed = true
    }

    private fun isRevoked(entry: Entry): Boolean = synchronized(lock) { entry.revoked || active !== entry }

    private fun requestClose(entry: Entry) {
        val launch = synchronized(lock) {
            if (active !== entry || entry.closeLaunched) false
            else {
                entry.revoked = true
                entry.state = "closing"
                entry.closeLaunched = true
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
                    rememberClosed(entry.id)
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
