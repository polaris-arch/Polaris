package com.polaris2.app.vpn

import java.util.concurrent.CompletableFuture
import java.util.concurrent.atomic.AtomicBoolean
import java.io.Closeable
import java.util.UUID

/** One main-core generation. Stop owns its close even when the factory has not returned yet. */
internal data class TunScope(
    val autoRoute: Boolean,
    val routes: List<String>,
    val excludedRoutes: List<String>,
    val skippedExcludes: List<String>,
    val allowedPackages: List<String>,
    val excludedPackages: List<String>,
    val skippedPackages: List<String>,
)

internal class MainKernelAttempt<Server>(
    val systemStartGeneration: Long = 0L,
    val runId: String = UUID.randomUUID().toString(),
) {
    /** Created by this attempt, never supplied by a bridge caller or reused after Service recreation. */
    val birthNonce: String = UUID.randomUUID().toString()
    private val closeLaunched = AtomicBoolean(false)
    /** Orders this generation's Start and Reload, without delaying Stop's terminal close. */
    val operationLock = Any()
    @Volatile var dualModeApiPort: Int? = null
    /** Set only by an explicit disconnect, never by failed-start cleanup. */
    @Volatile var clearReconnectNoticeOnClose = false
    @Volatile var revoked = false
        private set
    val prepared = CompletableFuture<Server?>()
    val closed = CompletableFuture<Throwable?>()
    /** Completed only after this exact owner has been removed from the process registry. */
    val released = CompletableFuture<Unit>()
    private var tun: Closeable? = null
    private var tunScope: TunScope? = null
    @Volatile private var startAcknowledged = false

    fun acknowledgeStart() = synchronized(this) {
        check(!revoked) { "旧主核已撤销，不能登记启动回执" }
        startAcknowledged = true
    }

    fun ownershipState(): String = when {
        revoked && closed.isDone && closed.getNow(null) != null -> "cleanupUnknown"
        revoked -> "closing"
        startAcknowledged -> "acknowledged"
        else -> "starting"
    }

    /** The caller also holds BoxService's short state lock. */
    fun installTun(value: Closeable, scope: TunScope): Closeable? = synchronized(this) {
        check(!revoked) { "旧主核已撤销，拒收迟到的 VPN fd" }
        val previous = tun
        tun = value
        tunScope = scope
        previous
    }

    fun currentTunScope(): TunScope? = synchronized(this) {
        if (revoked || tun == null) null else tunScope
    }

    /** The detached fd is closed by the Stop job outside BoxService's lock. */
    fun revokeAndDetachTun(): Closeable? = synchronized(this) {
        revoked = true
        val previous = tun
        tun = null
        tunScope = null
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

/** A Service can be recreated while the previous instance's native close is unresolved. */
internal val MainKernelAttemptRegistry = MainKernelAttemptLedger()

internal class MainKernelOwner(
    val attempt: MainKernelAttempt<*>,
    val requestClose: (() -> Unit)?,
)

internal data class MainKernelExactTarget(val runId: String, val birthNonce: String) {
    fun isValid(): Boolean = runId.isNotBlank() && runId == runId.trim() && runId.length <= 128 &&
        birthNonce.isNotBlank() && birthNonce == birthNonce.trim() && birthNonce.length <= 128
}

internal data class MainKernelExactResult(
    val target: MainKernelExactTarget,
    val state: String,
    val reason: String? = null,
)

internal data class MainKernelExactClose(
    val result: MainKernelExactResult,
    val owner: MainKernelOwner? = null,
)

internal class MainKernelAttemptLedger {
    private var owner: MainKernelAttempt<*>? = null
    private var ownerClose: (() -> Unit)? = null
    /** Only this process's confirmed native close may establish an AlreadyGone fact. */
    private var lastReleased: MainKernelExactTarget? = null

    private fun MainKernelAttempt<*>.exactTarget() = MainKernelExactTarget(runId, birthNonce)

    @Synchronized
    fun exactStatus(target: MainKernelExactTarget): MainKernelExactResult {
        if (!target.isValid()) return MainKernelExactResult(target, "Unknown", "invalid-target")
        val current = owner
        if (current?.exactTarget() == target) {
            return if (current.closed.isDone && current.closed.getNow(null) != null)
                MainKernelExactResult(target, "Unknown", "cleanup-unknown")
            else MainKernelExactResult(target, "Busy")
        }
        if (lastReleased == target) return MainKernelExactResult(target, "AlreadyGone")
        return MainKernelExactResult(target, "Unknown", if (current == null) "target-not-observed" else "different-owner")
    }

    /** Choose the exact attempt under the registry lock; invoke its callback only after releasing it. */
    @Synchronized
    fun exactCloseTarget(target: MainKernelExactTarget): MainKernelExactClose {
        val status = exactStatus(target)
        if (status.state != "Busy") return MainKernelExactClose(status)
        val current = owner ?: return MainKernelExactClose(MainKernelExactResult(target, "Unknown", "owner-lost"))
        val close = ownerClose ?: return MainKernelExactClose(MainKernelExactResult(target, "Unknown", "exact-close-unavailable"))
        return MainKernelExactClose(status, MainKernelOwner(current, close))
    }

    @Synchronized
    fun claim(attempt: MainKernelAttempt<*>, requestClose: (() -> Unit)? = null): Boolean {
        if (owner != null) return false
        owner = attempt
        ownerClose = requestClose
        return true
    }

    @Synchronized
    fun ownerForDrain(): MainKernelOwner? = owner?.let { MainKernelOwner(it, ownerClose) }

    /** Bridge Stop was already settled, but native close may still own this exact
     * attempt. Mark it before looking for a marker: a rejected SystemStart writes
     * its notice after finishStart, so Stop can arrive before that write. Only a
     * vacant registry may remove a marker observed under this same decision. */
    @Synchronized
    fun requestReconnectNoticeDismissal(
        readNoticeOwner: () -> String?,
        clearIfOwner: (String) -> Boolean,
    ): Boolean {
        val current = owner
        if (current != null) {
            current.clearReconnectNoticeOnClose = true
            return true
        }
        val markerOwner = readNoticeOwner() ?: return false
        return clearIfOwner(markerOwner)
    }

    @Synchronized
    fun isCurrent(attempt: MainKernelAttempt<*>): Boolean = owner === attempt

    @Synchronized
    fun isVacant(): Boolean = owner == null

    /** "acknowledged" is a start result, not a post-start process liveness probe. */
    @Synchronized
    fun snapshot(): Pair<String, String?> = owner?.let { it.ownershipState() to it.runId } ?: ("absent" to null)

    /** Registry identity and fd publication are one decision, including Service recreation. */
    @Synchronized
    fun installTun(attempt: MainKernelAttempt<*>, value: Closeable, scope: TunScope): Closeable? {
        check(owner === attempt) { "旧主核已失去 TUN 所有权" }
        return attempt.installTun(value, scope)
    }

    /** No factory was started, so this rejected system intent owns no native server. */
    fun abandon(attempt: MainKernelAttempt<*>): Boolean {
        val abandoned = synchronized(this) {
            if (owner !== attempt || !attempt.prepared.isDone || attempt.prepared.getNow(null) != null) false
            else {
                owner = null
                ownerClose = null
                true
            }
        }
        if (abandoned) attempt.released.complete(Unit)
        return abandoned
    }

    /** Keep the bridge acknowledgement and owner release atomic against a new Service claim. */
    fun completeAfterClose(attempt: MainKernelAttempt<*>, action: () -> Unit): Boolean {
        val completed = synchronized(this) {
            if (owner !== attempt || !attempt.closed.isDone || attempt.closed.getNow(null) != null) false
            else {
                action()
                lastReleased = attempt.exactTarget()
                owner = null
                ownerClose = null
                true
            }
        }
        if (completed) attempt.released.complete(Unit)
        return completed
    }
}
