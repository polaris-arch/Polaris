package com.polaris2.app.vpn

import java.util.concurrent.CompletableFuture
import java.util.concurrent.atomic.AtomicBoolean
import java.io.Closeable

/** One main-core generation. Stop owns its close even when the factory has not returned yet. */
internal class MainKernelAttempt<Server>(val systemStartGeneration: Long = 0L) {
    private val closeLaunched = AtomicBoolean(false)
    /** Orders this generation's Start and Reload, without delaying Stop's terminal close. */
    val operationLock = Any()
    @Volatile var revoked = false
        private set
    val prepared = CompletableFuture<Server?>()
    val closed = CompletableFuture<Throwable?>()
    private var tun: Closeable? = null

    /** The caller also holds BoxService's short state lock. */
    fun installTun(value: Closeable): Closeable? = synchronized(this) {
        check(!revoked) { "旧主核已撤销，拒收迟到的 VPN fd" }
        val previous = tun
        tun = value
        previous
    }

    /** The detached fd is closed by the Stop job outside BoxService's lock. */
    fun revokeAndDetachTun(): Closeable? = synchronized(this) {
        revoked = true
        val previous = tun
        tun = null
        previous
    }
    fun publish(server: Server) { check(prepared.complete(server)) }
    fun skipPreparation() { prepared.complete(null) }

    /** The job is never cancelled by an IPC timeout; late publication still reaches this close. */
    fun closeOnce(action: (Server?) -> Unit) {
        if (!closeLaunched.compareAndSet(false, true)) return
        Thread({
            val failure = runCatching { action(prepared.get()) }.exceptionOrNull()
            closed.complete(failure)
        }, "polaris-main-close").start()
    }
}
