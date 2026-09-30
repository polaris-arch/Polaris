package com.polaris2.app.vpn

import com.polaris2.app.BuildConfig
import java.io.Closeable
import app.tauri.annotation.InvokeArg

@InvokeArg
class DebugCoreProbeSessionArgs { lateinit var sessionId: String }

@InvokeArg
class DebugCoreProbeLoanEnvelopeArgs { lateinit var loan: DebugCoreProbeLoanArgs }

/** Native-only wire type. Do not add a JS command taking this type or format it in an error. */
class DebugCoreProbeLoanArgs {
    lateinit var bootNonce: String
    lateinit var sessionId: String
    lateinit var nonce: String
    lateinit var planSha256: String
    lateinit var apkSha256: String
    lateinit var expectedSourcePin: String
    lateinit var generation: String
    lateinit var runId: String
    lateinit var birthNonce: String
    lateinit var configDigest: String
    var revision: Long = 0
    var deadlineElapsed: Long = 0
    var probePort: Int = 0
    var expiresElapsed: Long = 0
    var password: ByteArray = byteArrayOf()
    override fun toString() = "DebugCoreProbeLoanArgs(<private>)"
}

/** Pure credential bytes remain an exact existing-lease handle while queued or revoked. */
internal class DebugCoreProbeCredentialBuffer(private val bytes: ByteArray) : Closeable {
    fun erase() { bytes.fill(0) }
    override fun close() = erase()
    override fun toString() = "DebugCoreProbeCredentialBuffer(<private>)"
}

/**
 * One private credential resource in an existing batch lease. This is not a Main owner or a
 * transport factory. Only the native Rust publication provider supplies a trusted generation.
 * PC ACK/transport is not connected: the private manager admits and erases this resource only.
 */
internal class DebugCoreProbeLoan private constructor(
    private val scope: Scope,
    private val binding: Binding,
    private val input: CurrentInput,
    private val lease: DebugBatchLease,
    private val ticket: DebugBatchLease.CommandTicket,
    private val credential: ByteArray,
) : Closeable {
    /** Frozen by the original batch and its native Rust generation admission, never by JS. */
    data class Scope(
        val bootNonce: String, val sessionId: String, val nonce: String, val planSha256: String,
        val apkSha256: String, val expectedSourcePin: String, val generation: String,
        val runId: String, val birthNonce: String, val revision: Long,
        val configDigest: String, val deadlineElapsed: Long,
    )

    /** Typed native-only descriptor. The actual probe port is separate from the approved LAN port. */
    data class Binding(val scope: Scope, val probePort: Int, val expiresElapsed: Long)

    /** A bounded metadata read from the actual Main operationLock and witness reference pair. */
    class CurrentInput internal constructor(
        internal val owner: MainKernelAttempt<*>, internal val server: Any,
        internal val snapshot: DebugAppliedInputWitness.Snapshot,
        internal val registry: MainKernelAttemptLedger, internal val witness: DebugAppliedInputWitness,
    )

    private var used = false
    @Volatile private var cleared = false

    /** No callback under this short lock. Session seal can erase the bytes during a blocked write. */
    fun consume(now: Long, current: CurrentInput?, write: (ByteArray) -> Unit) {
        check(BuildConfig.DEBUG) { "Debug core probe loan is disabled" }
        try {
            val admitted = synchronized(input.owner.operationLock) {
                val fresh = current?.takeIf { it.owner === input.owner && it.server === input.server }
                    ?.let(::refreshInput)
                synchronized(this) {
                    check(!used && !cleared && isCurrent(now, fresh)) { "Core probe loan unavailable" }
                    used = true
                    credential
                }
            }
            write(admitted)
        } finally { lease.retire(this) }
    }

    /** Before and after transport work: owner/revision loss never proves a successful core path. */
    fun isCurrent(now: Long, current: CurrentInput?): Boolean =
        !cleared && now >= 0 && now < binding.expiresElapsed && now < scope.deadlineElapsed &&
        lease.ownsProbe(ticket) && lease.ownsOpenHandle(this) && current != null &&
        current.owner === input.owner && current.server === input.server && matches(scope, current)

    /** Pure byte fence; callable before any unrelated original-lease Close can block. */
    @Synchronized fun erase() {
        credential.fill(0)
        cleared = true
    }

    override fun close() = erase()

    override fun toString() = "DebugCoreProbeLoan(<private>)"

    companion object {
        /** No JNI, SDK, hashing or wait. Do not hold any batch/publication gate around this call. */
        fun currentInput(
            registry: MainKernelAttemptLedger = MainKernelAttemptRegistry,
            witness: DebugAppliedInputWitness = DebugAppliedInputs.witness,
        ): CurrentInput? {
            check(BuildConfig.DEBUG) { "Debug core probe loan is disabled" }
            val owner = registry.ownerForDrain()?.attempt ?: return null
            return synchronized(owner.operationLock) {
                if (owner.revoked || !registry.isCurrent(owner) || !owner.prepared.isDone ||
                    owner.prepared.isCompletedExceptionally) return@synchronized null
                val server = owner.prepared.getNow(null) ?: return@synchronized null
                val actual = witness.snapshotFor(owner, server) ?: return@synchronized null
                CurrentInput(owner, server, actual, registry, witness)
            }
        }

        /** Recheck while holding the same actual operationLock; do not wait on another owner. */
        private fun refreshInput(input: CurrentInput): CurrentInput? {
            check(Thread.holdsLock(input.owner.operationLock))
            if (input.owner.revoked || !input.registry.isCurrent(input.owner) ||
                !input.owner.prepared.isDone || input.owner.prepared.isCompletedExceptionally ||
                input.owner.prepared.getNow(null) !== input.server) return null
            val actual = input.witness.snapshotFor(input.owner, input.server) ?: return null
            return CurrentInput(input.owner, input.server, actual, input.registry, input.witness)
        }

        /**
         * Transfer the one incoming buffer into the original lease. Every rejected/late transfer
         * clears it; publication after seal closes it through the existing exact handle ledger.
         * This performs no socket allocation. A missing publication generation is rejected.
         */
        fun admit(scope: Scope, binding: Binding, actual: CurrentInput?,
                  lease: DebugBatchLease, ticket: DebugBatchLease.CommandTicket,
                  now: Long, password: ByteArray): DebugCoreProbeLoan {
            try {
                check(BuildConfig.DEBUG) { "Debug core probe loan is disabled" }
                checkNotNull(actual) { "Core probe Main input unavailable" }
                return synchronized(actual.owner.operationLock) {
                    val fresh = checkNotNull(refreshInput(actual)) { "Core probe Main input changed" }
                    require(validScope(scope) && scope == binding.scope && binding.probePort in 1..65535 &&
                        now >= 0 && binding.expiresElapsed > now && binding.expiresElapsed <= scope.deadlineElapsed &&
                        scope.deadlineElapsed - now <= 300000 && matches(scope, fresh)) {
                        "Core probe binding unavailable"
                    }
                    // The current core uses a fixed username and a 128-bit lowercase hex password.
                    require(password.size == 32 && password.all { it.toInt() in 48..57 || it.toInt() in 97..102 }) {
                        "Core probe credential unavailable"
                    }
                    check(lease.ownsProbe(ticket) && lease.beginAcquire()) { "Original probe command unavailable" }
                    val loan = try { DebugCoreProbeLoan(scope, binding, fresh, lease, ticket, password) }
                        catch (failure: Throwable) { lease.acquireFailed(); throw failure }
                    check(lease.publish(loan)) { "Original probe command sealed" }
                    loan
                }
            } catch (failure: Throwable) {
                password.fill(0)
                throw failure
            }
        }

        private fun validScope(scope: Scope): Boolean =
            scope.bootNonce.matches(Regex("[0-9a-f]{32}")) && scope.sessionId.matches(Regex("[0-9a-f]{32}")) &&
            scope.nonce.matches(Regex("[0-9a-f]{48}")) && scope.planSha256.matches(Regex("[0-9a-f]{64}")) &&
            scope.apkSha256.matches(Regex("[0-9a-f]{64}")) && scope.expectedSourcePin.matches(Regex("[0-9a-f]{64}")) &&
            scope.generation.matches(Regex("[1-9][0-9]{0,19}")) && scope.generation.toULongOrNull() != null &&
            MainKernelExactTarget(scope.runId, scope.birthNonce).isValid() && scope.revision > 0 &&
            scope.configDigest.matches(Regex("[0-9a-f]{64}")) && scope.deadlineElapsed > 0

        private fun matches(scope: Scope, input: CurrentInput): Boolean =
            !input.owner.revoked && input.owner.runId == scope.runId && input.owner.birthNonce == scope.birthNonce &&
            input.snapshot.runId == scope.runId && input.snapshot.birthNonce == scope.birthNonce &&
            input.snapshot.revision == scope.revision && input.snapshot.configDigest == scope.configDigest &&
            input.snapshot.stage == "NativeInputReturned" && input.snapshot.startAcknowledged
    }
}
