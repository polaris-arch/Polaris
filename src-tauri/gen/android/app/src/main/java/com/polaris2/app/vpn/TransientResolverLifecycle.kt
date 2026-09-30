package com.polaris2.app.vpn

import java.util.concurrent.CancellationException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/** Metadata only: SDK callbacks own mailboxes, while the original call alone delivers to JNI. */
internal class TransientResolverLifecycle(
    private val cancellationExecutor: Executor = cancellationWorkers,
) {
    class CleanupUnknown : IllegalStateException("android: DNS callback cleanup unknown")
    enum class Drain { UnusedClosed, Unknown }
    private enum class State { Pending, Ready, Cancelled, Delivering, Delivered, Returned }
    data class Snapshot(
        val sealed: Boolean,
        val queried: Boolean,
        val calls: Int,
        val deliveries: Int,
        val cancellationTasks: Int,
        val sdkTasks: Int,
        val sdkSubmissions: Long,
        val cancellationUnknown: Boolean,
    ) {
        // Only work observed locally; late SDK tasks can arrive after this observation.
        val locallyDrained get() = sealed && calls == 0 && deliveries == 0 && cancellationTasks == 0 && sdkTasks == 0
    }

    private val gate = Any()
    private var sealed = false
    private var queried = false
    private val queries = mutableSetOf<Query<*>>()
    private var deliveries = 0
    private var cancellationTasks = 0
    private var sdkTasks = 0
    private var sdkSubmissions = 0L
    private var cancellationUnknown = false
    private var changed = CountDownLatch(1)

    private fun <T> update(action: () -> T): T {
        var wake: CountDownLatch? = null
        try {
            return synchronized(gate) {
                wake = changed
                changed = CountDownLatch(1)
                action()
            }
        } finally {
            wake?.countDown()
        }
    }

    fun <T : Any> enterQuery(): Query<T> = update {
        check(!sealed) { "android: DNS transport is closed" }
        queried = true
        Query<T>(this).also { queries.add(it) }
    }

    internal class CancelTask(val action: () -> Unit)

    private fun dispatch(task: CancelTask?) {
        if (task == null) return
        try {
            cancellationExecutor.execute {
                try { task.action() }
                catch (_: Throwable) { update { cancellationUnknown = true } }
                finally { update { cancellationTasks-- } }
            }
        } catch (_: Throwable) {
            // Rejection is not completion. Keep the reserved task visible as Unknown.
            update { cancellationUnknown = true }
        }
    }

    /** No proof wait: revoke first, wake every original call, then dispatch best-effort SDK cancel. */
    fun beginClose() {
        val revoked = update {
            sealed = true
            queries.map { it to it.revokeLocked() }
        }
        revoked.forEach { (query, _) -> query.wake() }
        revoked.forEach { (_, task) -> dispatch(task) }
    }

    fun closeUnused() {
        beginClose()
        if (snapshot().queried) throw CleanupUnknown()
    }

    fun snapshot(): Snapshot = synchronized(gate) { snapshotLocked() }
    private fun snapshotLocked() = Snapshot(sealed, queried, queries.size, deliveries,
        cancellationTasks, sdkTasks, sdkSubmissions, cancellationUnknown)

    /** An independent bounded observation. Timeout never removes a query, permit or SDK task. */
    fun awaitLocalDrain(timeout: Long, unit: TimeUnit): Drain {
        val budget = unit.toNanos(timeout).coerceAtLeast(0)
        val started = System.nanoTime()
        while (true) {
            val (snapshot, event) = synchronized(gate) { snapshotLocked() to changed }
            if (snapshot.locallyDrained) {
                return if (!snapshot.queried && !snapshot.cancellationUnknown) Drain.UnusedClosed else Drain.Unknown
            }
            val remaining = budget - (System.nanoTime() - started)
            if (remaining <= 0) return Drain.Unknown
            try { event.await(remaining, TimeUnit.NANOSECONDS) }
            catch (_: InterruptedException) {
                Thread.currentThread().interrupt()
                return Drain.Unknown
            }
        }
    }

    /** Never seal or drop SDK Runnables: they also read resolver results and clean up framework fds. */
    fun sdkExecutor(delegate: Executor): Executor = Executor { runnable ->
        update { sdkTasks++ }
        delegate.execute {
            try { runnable.run() }
            finally { update { sdkTasks-- } }
        }
        // If dispatch throws, the task stays in flight until it actually runs (if ever).
    }

    class Query<T : Any> internal constructor(private val owner: TransientResolverLifecycle) {
        private var state = State.Pending
        private var value: T? = null
        private var cancelled = false
        private var cancellation: (() -> Unit)? = null
        private var cancellationDispatched = false
        private val ready = CountDownLatch(1)

        private fun reserveCancellationLocked(): CancelTask? {
            val action = cancellation ?: return null
            if (cancellationDispatched) return null
            cancellationDispatched = true
            cancellation = null
            owner.cancellationTasks++
            return CancelTask(action)
        }

        internal fun revokeLocked(): CancelTask? {
            if (state == State.Returned) return null
            cancelled = true
            if (state == State.Pending || state == State.Ready) {
                state = State.Cancelled
                value = null
            }
            return reserveCancellationLocked()
        }

        internal fun wake() = ready.countDown()

        fun installCancellation(action: () -> Unit) {
            val task = owner.update {
                check(cancellation == null && !cancellationDispatched && state != State.Returned)
                cancellation = action
                if (cancelled) reserveCancellationLocked() else null
            }
            owner.dispatch(task)
        }

        /** Record entry before calling SDK: a partial submission throw is not a no-resource fact. */
        fun enterSdkSubmission(): Boolean = owner.update {
            if (owner.sealed || cancelled || state != State.Pending) false
            else { owner.sdkSubmissions++; true }
        }

        fun canStartBlockingLookup(): Boolean = synchronized(owner.gate) {
            !owner.sealed && !cancelled && state == State.Pending
        }

        /** Callbacks only publish a value. Duplicates, revoked Ready values and late results are discarded. */
        fun publish(result: T): Boolean {
            val accepted = owner.update {
                if (owner.sealed || cancelled || state != State.Pending) false
                else { value = result; state = State.Ready; true }
            }
            if (accepted) ready.countDown()
            return accepted
        }

        fun cancel() {
            val task = owner.update { revokeLocked() }
            ready.countDown()
            owner.dispatch(task)
        }

        /** Only the exchange/lookup stack uses this method; no JNI or waits occur under the metadata gate. */
        fun awaitAndDeliver(action: (T) -> Unit) {
            val result = try {
                ready.await()
                owner.update {
                    // Monitor acquisition is not interruptible. Recheck before granting the permit.
                    if (Thread.currentThread().isInterrupted) null
                    else {
                        if (owner.sealed || cancelled || state == State.Cancelled) {
                            throw CancellationException("android: DNS query cancelled or transport closed")
                        }
                        check(state == State.Ready) { "android: DNS result already delivered" }
                        val result = checkNotNull(value)
                        value = null
                        state = State.Delivering
                        owner.deliveries++
                        result
                    }
                } ?: throw InterruptedException("android: DNS query interrupted before delivery")
            }
            catch (error: InterruptedException) {
                cancel()
                Thread.currentThread().interrupt()
                throw error
            }
            try { action(result) }
            finally { owner.update { owner.deliveries--; state = State.Delivered } }
        }

        /** The call is returned only after any permitted JNI delivery really exits. */
        fun returned() = owner.update {
            check(state != State.Delivering) { "android: DNS delivery is still in flight" }
            if (state != State.Returned) {
                state = State.Returned
                value = null
                cancellation = null
                owner.queries.remove(this)
            }
        }
    }

    companion object {
        private val cancellationWorkers = Executors.newCachedThreadPool { runnable ->
            Thread(runnable, "polaris-dns-cancel").apply { isDaemon = true }
        }
    }
}

/** Go's current OnCancel registration is not unregistered by this; only its local ability is cleared. */
internal class TransientResolverCancelHook<T : Any>(query: TransientResolverLifecycle.Query<T>) {
    private val target = AtomicReference<TransientResolverLifecycle.Query<T>?>(query)
    fun cancel() { target.get()?.cancel() }
    fun clear() { target.set(null) }
}
