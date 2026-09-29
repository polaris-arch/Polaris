package com.polaris2.app.vpn

import java.util.concurrent.TimeUnit

/**
 * Process-local handoff before a durable Preparing marker exists. Every legacy
 * bridge request and Service attempt claim enters through [admit]. The fence
 * itself performs no I/O, native call, callback delivery, or close while held.
 *
 * Lock order: BoxService -> this gate -> VpnBridge/registry, and SystemStart ->
 * this gate. Callers must never acquire BoxService or SystemStart from the gate.
 */
internal class LegacyAdmissionFence<Pending>(
    private val detachPending: () -> Pending?,
    private val bridgeIdle: () -> Boolean,
    private val currentOwner: () -> MainKernelOwner?,
) {
    private val gate = Any()
    private var fence: Fence? = null

    private data class Fence(val id: String, val owner: MainKernelOwner?)

    data class Beginning<Pending>(
        val pendingToReject: Pending?,
        val ownerToClose: MainKernelOwner?,
    )

    data class Status(
        val fenceId: String,
        val state: String,
        val runId: String? = null,
        val closedRunId: String? = null,
        val reason: String? = null,
    )

    data class Admission<T>(val rejectedByFence: Boolean, val value: T?)

    fun <T> admitWithDecision(action: () -> T): Admission<T> = synchronized(gate) {
        if (fence != null) Admission(true, null) else Admission(false, action())
    }

    fun <T> admit(action: () -> T): T? = admitWithDecision(action).value

    /** A pre-gate request snapshot may have been replaced while the Service was queued. */
    fun <Request, T> admitCurrentRequest(request: Request?, currentRequest: () -> Request?, action: () -> T): Admission<T?> =
        admitWithDecision { if (currentRequest() !== request) null else action() }

    fun requireOpen() = synchronized(gate) {
        check(fence == null) { "android: legacy system start 已被受管迁移屏障阻断" }
    }

    /** Same ID is retryable; a different ID cannot replace a still-live fence. */
    fun begin(id: String): Beginning<Pending> = synchronized(gate) {
        require(id.isNotBlank() && id.length <= 128 && id == id.trim()) { "invalid legacy fence ID" }
        val existing = fence
        if (existing != null) {
            check(existing.id == id) { "another legacy fence is active" }
            return@synchronized Beginning(null, existing.owner)
        }
        // Publish the closed gate before examining either source. All producers
        // use this monitor, so a pre-Service request cannot slip past the snapshot.
        fence = Fence(id, null)
        val pending = detachPending()
        val owner = currentOwner()
        fence = Fence(id, owner)
        Beginning(pending, owner)
    }

    fun status(id: String): Status = synchronized(gate) {
        val active = fence
            ?: return@synchronized Status(id, "unknown", reason = "fence-not-acquired")
        if (active.id != id) return@synchronized Status(id, "unknown", reason = "fence-id-mismatch")
        val ownerNow = currentOwner()
        val idle = bridgeIdle()
        val prior = active.owner
        if (prior == null) {
            return@synchronized if (ownerNow == null && idle) Status(id, "vacant")
            else Status(id, "unknown", reason = "unaccounted-owner-or-bridge-request")
        }
        val runId = prior.attempt.runId
        if (ownerNow != null && ownerNow.attempt !== prior.attempt) {
            return@synchronized Status(id, "unknown", runId, reason = "owner-identity-changed")
        }
        if (prior.attempt.closeFailure() != null) {
            return@synchronized Status(id, "unknown", runId, reason = "cleanup-unknown")
        }
        if (ownerNow == null && prior.attempt.released.isDone && idle) {
            return@synchronized Status(id, "vacant", closedRunId = runId)
        }
        if (ownerNow == null || prior.requestClose == null) {
            return@synchronized Status(id, "unknown", runId, reason = "exact-close-unavailable")
        }
        Status(id, "owned", runId)
    }

    /** Bounded wait; a timeout leaves the fence and exact owner intact. */
    fun await(id: String, timeout: Long, unit: TimeUnit): Status {
        val owner = synchronized(gate) { fence?.takeIf { it.id == id }?.owner }
        if (owner != null && !owner.attempt.released.isDone) {
            runCatching { owner.attempt.released.get(timeout, unit) }
        }
        val result = status(id)
        return if (result.state == "owned") result.copy(state = "unknown", reason = "close-not-confirmed")
        else result
    }
}
