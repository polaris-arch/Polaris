package com.polaris2.app.vpn

/** Observation only. No native/resource ownership, callbacks, I/O or SDK work under this gate. */
internal class DebugAppliedInputWitness {
    data class Token internal constructor(val revision: Long, internal val owner: Any, internal val server: Any)
    data class Snapshot(val runId: String?, val birthNonce: String?, val configDigest: String?,
                        val revision: Long, val stage: String, val startAcknowledged: Boolean)
    private val gate = Any()
    private var revision = 0L
    private var current: Token? = null
    private var value = Snapshot(null, null, null, 0, "Unknown", false)

    fun begin(owner: Any, server: Any, runId: String, birthNonce: String, digest: String): Token = synchronized(gate) {
        check(revision < Long.MAX_VALUE)
        val token = Token(++revision, owner, server)
        val acknowledged = current?.owner === owner && value.startAcknowledged
        current = token
        value = Snapshot(runId, birthNonce, digest, token.revision, "NativeInputInFlight", acknowledged)
        token
    }
    fun returned(token: Token, stillCurrent: Boolean) = synchronized(gate) {
        if (current !== token) return@synchronized
        value = value.copy(stage = if (stillCurrent) "NativeInputReturned" else "Unknown")
        if (!stillCurrent) current = null
    }
    fun acknowledge(owner: Any, server: Any, accepted: Boolean) = synchronized(gate) {
        val token = current
        if (token?.owner === owner && token.server === server && value.stage == "NativeInputReturned") {
            value = value.copy(startAcknowledged = accepted, stage = if (accepted) value.stage else "Unknown")
            if (!accepted) current = null
        }
    }
    fun failed(token: Token) = synchronized(gate) {
        if (current === token) { value = value.copy(stage = "Unknown"); current = null }
    }
    fun seal(owner: Any) = synchronized(gate) {
        if (current?.owner === owner) { value = value.copy(stage = "Unknown", startAcknowledged = false); current = null }
    }
    fun snapshot(): Snapshot = synchronized(gate) { value.copy() }

    /** An actual Main operationLock caller may bind the observation to both live references. */
    fun snapshotFor(owner: Any, server: Any): Snapshot? = synchronized(gate) {
        val token = current
        if (token?.owner === owner && token.server === server &&
            value.stage == "NativeInputReturned" && value.startAcknowledged) value.copy() else null
    }
}

internal object DebugAppliedInputs { val witness = DebugAppliedInputWitness() }
