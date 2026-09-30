package com.polaris2.app.vpn

import java.io.Closeable
import java.util.Collections
import java.util.IdentityHashMap
import java.util.concurrent.atomic.AtomicBoolean

/** Exact handles survive cancellation/late acquire. Close/wait/factories never run under gate. */
internal class DebugBatchLease {
    data class Snapshot(val sealed: Boolean, val workers: Int, val acquiring: Int,
                        val closing: Int, val handles: Int, val closeFailed: Boolean,
                        val commands: Int, val residualHandles: Int)
    class CommandTicket internal constructor(internal val action: String)
    private val gate = Any()
    private var sealed = false
    private var workers = 0
    private var acquiring = 0
    private var closing = 0
    private var failed = false
    // Strong identity custody lasts through Close and every uncertain result.
    private val handles = Collections.newSetFromMap(IdentityHashMap<Closeable, Boolean>())
    private val closingHandles = Collections.newSetFromMap(IdentityHashMap<Closeable, Boolean>())
    private val residualHandles = Collections.newSetFromMap(IdentityHashMap<Closeable, Boolean>())
    private val commands = IdentityHashMap<CommandTicket, Boolean>()
    fun workerBorn(): Boolean = synchronized(gate) { if (sealed) false else { workers++; true } }
    fun workerReturned() = synchronized(gate) { check(workers > 0); workers-- }
    fun beginAcquire(): Boolean = synchronized(gate) { if (sealed) false else { acquiring++; true } }
    fun publish(value: Closeable): Boolean {
        val accepted = synchronized(gate) {
            check(acquiring > 0); acquiring--; check(handles.add(value))
            if (sealed) { closingHandles.add(value); closing++; false } else true
        }
        if (!accepted) closeHandle(value)
        return accepted
    }
    fun acquireFailed() = synchronized(gate) { check(acquiring > 0); acquiring-- }
    fun retire(value: Closeable) {
        val owned = synchronized(gate) {
            if (handles.contains(value) && !residualHandles.contains(value) && closingHandles.add(value)) { closing++; true } else false
        }
        if (owned) closeHandle(value)
    }
    /** JNI-input/Stop metadata fence. Only known pure byte erasure here, no transport Close. */
    fun revokeProbeCredentials() {
        val credentials = synchronized(gate) {
            sealed = true
            handles.filter { it is DebugCoreProbeCredentialBuffer || it is DebugCoreProbeLoan }
        }
        credentials.forEach(::eraseProbeCredential)
    }
    fun seal() {
        val (owned, credentials) = synchronized(gate) {
            sealed = true
            val owned = handles.filter { !residualHandles.contains(it) && !closingHandles.contains(it) }
                .also { closing += it.size; closingHandles.addAll(it) }
            owned to handles.filter { it is DebugCoreProbeCredentialBuffer || it is DebugCoreProbeLoan }
        }
        // Pure typed byte erasure precedes every arbitrary Close. Custody/tickets remain owned;
        // no lease gate is held while taking the existing short loan byte lock or calling Close.
        credentials.forEach(::eraseProbeCredential)
        owned.forEach(::closeHandle)
    }
    private fun eraseProbeCredential(value: Closeable) {
        when (value) {
            is DebugCoreProbeCredentialBuffer -> value.erase()
            is DebugCoreProbeLoan -> value.erase()
        }
    }
    private fun closeHandle(value: Closeable) {
        val error = runCatching { value.close() }.isFailure
        synchronized(gate) {
            failed = failed || error
            if (error) residualHandles.add(value) else handles.remove(value)
            check(closingHandles.remove(value)); closing--
        }
    }
    fun commandBorn(action: String = "snapshot"): CommandTicket? = synchronized(gate) {
        if (sealed && action !in setOf("close", "cleanupObserve")) null else CommandTicket(action).also { commands[it] = false }
    }
    fun commandEntered(ticket: CommandTicket): Boolean = synchronized(gate) {
        if (!commands.containsKey(ticket)) false else { check(commands[ticket] == false); commands[ticket] = true; true }
    }
    fun commandReturned(ticket: CommandTicket) = synchronized(gate) { check(commands.remove(ticket) != null) }
    /** Private resource loans use the original entered probe ticket, never a new command ledger. */
    fun ownsProbe(ticket: CommandTicket): Boolean = synchronized(gate) {
        !sealed && ticket.action == "probe" && commands[ticket] == true
    }
    fun ownsOpenHandle(value: Closeable): Boolean = synchronized(gate) {
        !sealed && handles.contains(value) && !closingHandles.contains(value) && !residualHandles.contains(value)
    }
    fun snapshot() = snapshotFor(null)
    /** Only the final pure metadata reporter may exclude its own entered ticket. */
    fun snapshotFor(reporter: CommandTicket?) = synchronized(gate) {
        val self = if (reporter?.action == "cleanupObserve" && commands[reporter] == true) 1 else 0
        Snapshot(sealed, workers, acquiring, closing, handles.size, failed, commands.size - self, residualHandles.size)
    }
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

/** The immutable original command ticket is admitted before queueing, returned after real callbacks. */
internal class DebugBatchCommandTask(private val lease: DebugBatchLease,
                                     private val ticket: DebugBatchLease.CommandTicket,
                                     private val action: (DebugBatchLease.CommandTicket) -> Unit,
                                     private val failure: (Throwable) -> Unit) : Runnable {
    private val started = AtomicBoolean(false)
    override fun run() {
        if (!started.compareAndSet(false, true) || !lease.commandEntered(ticket)) return
        try { action(ticket) } catch (error: Throwable) { failure(error) } finally { lease.commandReturned(ticket) }
    }
    fun rejectBeforeRun(error: Throwable) {
        if (!started.compareAndSet(false, true) || !lease.commandEntered(ticket)) return
        try { failure(error) } finally { lease.commandReturned(ticket) }
    }
}
