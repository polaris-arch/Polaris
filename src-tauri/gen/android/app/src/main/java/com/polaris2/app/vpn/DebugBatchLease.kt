package com.polaris2.app.vpn

import java.io.Closeable
import java.util.concurrent.atomic.AtomicBoolean

/** Exact handles survive cancellation/late acquire. Close/wait/factories never run under gate. */
internal class DebugBatchLease {
    data class Snapshot(val sealed: Boolean, val workers: Int, val acquiring: Int,
                        val closing: Int, val handles: Int, val closeFailed: Boolean)
    private val gate = Any()
    private var sealed = false
    private var workers = 0
    private var acquiring = 0
    private var closing = 0
    private var failed = false
    private val handles = mutableSetOf<Closeable>()
    fun workerBorn(): Boolean = synchronized(gate) { if (sealed) false else { workers++; true } }
    fun workerReturned() = synchronized(gate) { check(workers > 0); workers-- }
    fun beginAcquire(): Boolean = synchronized(gate) { if (sealed) false else { acquiring++; true } }
    fun publish(value: Closeable): Boolean {
        val accepted = synchronized(gate) { check(acquiring > 0); acquiring--; if (sealed) { closing++; false } else handles.add(value) }
        if (!accepted) closeHandle(value)
        return accepted
    }
    fun acquireFailed() = synchronized(gate) { check(acquiring > 0); acquiring-- }
    fun retire(value: Closeable) {
        val owned = synchronized(gate) { if (handles.remove(value)) { closing++; true } else false }
        if (owned) closeHandle(value)
    }
    fun seal() {
        val owned = synchronized(gate) {
            sealed = true
            handles.toList().also { closing += it.size; handles.clear() }
        }
        owned.forEach(::closeHandle)
    }
    private fun closeHandle(value: Closeable) {
        val error = runCatching { value.close() }.isFailure
        synchronized(gate) { failed = failed || error; closing-- }
    }
    fun snapshot() = synchronized(gate) { Snapshot(sealed, workers, acquiring, closing, handles.size, failed) }
}

/** Queued cancellation returns its worker admission; a running task returns only in actual finally. */
internal class DebugBatchTask(private val lease: DebugBatchLease, private val action: () -> Unit,
                             private val failure: (Throwable) -> Unit) : Runnable {
    private val started = AtomicBoolean(false)
    override fun run() {
        if (!started.compareAndSet(false, true)) return
        try { action() } catch (error: Throwable) { failure(error) } finally { lease.workerReturned() }
    }
    fun rejectBeforeRun() { if (started.compareAndSet(false, true)) lease.workerReturned() }
}
