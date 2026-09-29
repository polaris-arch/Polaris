package com.polaris2.app.vpn

import android.content.Context
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.util.UUID

/** The result of one cold-process stat. Only ENOENT opens native admission. */
internal enum class RequiredMarkerProof { Absent, PresentOrUnknown }

/** Frozen membership plus live terminal facts; no field on its own asserts NoOldCore. */
internal data class AndroidDrainReceipt(
    val processNonce: String,
    val fenceId: String,
    val markerProof: RequiredMarkerProof?,
    val sealedRevision: Long,
    val revision: Long,
    val captured: List<AndroidNativeAdmission.Entry>,
)

/** Pure process ledger. Its monitor protects only state; callers do all I/O and JNI outside it. */
internal class AndroidNativeAdmission(
    val processNonce: String = UUID.randomUUID().toString(),
) {
    enum class Kind { Main, Login, Speedtest, CheckConfig, TargetlessStop, TargetlessReload }
    enum class State { Reserved, BirthEntered, CancelledBeforeBirth, ClosedExact, Completed, Unknown, ValidationCleanupUnknown }
    data class Ticket(val id: String, val kind: Kind, val logicalId: String)
    data class Entry(val ticket: Ticket, val state: State)
    class AdmissionClosed : IllegalStateException("android: native admission is closed")

    private val lock = Any()
    private var bootstrap: RequiredMarkerProof? = null
    private var fenceId: String? = null
    private var sealedRevision = 0L
    private var revision = 0L
    private val entries = LinkedHashMap<String, Entry>()
    private val usedOwners = HashMap<Pair<Kind, String>, Ticket?>()
    private var captured = emptyList<String>()

    /** Called once after a marker stat made outside [lock]. An uncertain result never opens. */
    fun bootstrap(proof: RequiredMarkerProof) = synchronized(lock) {
        check(bootstrap == null) { "native admission already bootstrapped" }
        bootstrap = proof
        revision++
    }

    fun reserveOwner(kind: Kind, logicalId: String): Ticket {
        require(kind in OWNER_KINDS)
        return reserve(kind, logicalId, owner = true)
    }
    fun reserveOperation(kind: Kind): Ticket {
        require(kind !in OWNER_KINDS)
        return reserve(kind, UUID.randomUUID().toString(), owner = false)
    }

    private fun reserve(kind: Kind, logicalId: String, owner: Boolean): Ticket = synchronized(lock) {
        if (fenceId != null || bootstrap != RequiredMarkerProof.Absent) throw AdmissionClosed()
        require(logicalId.isNotBlank()) { "native owner identity is empty" }
        if (owner) check(!usedOwners.containsKey(kind to logicalId)) { "native owner identity was already consumed" }
        val ticket = Ticket(UUID.randomUUID().toString(), kind, logicalId)
        if (owner) usedOwners[kind to logicalId] = ticket
        check(entries.put(ticket.id, Entry(ticket, State.Reserved)) == null)
        revision++
        ticket
    }

    /** A worker must call this immediately before crossing a native factory/JNI boundary. */
    fun enterBirth(ticket: Ticket): Boolean = synchronized(lock) {
        val current = entries[ticket.id] ?: return@synchronized false
        if (current.ticket != ticket || current.state != State.Reserved || fenceId != null) return@synchronized false
        entries[ticket.id] = current.copy(state = State.BirthEntered)
        revision++
        true
    }

    /** A reservation cancelled before native birth is a positive terminal fact. */
    fun cancelBeforeBirth(ticket: Ticket): Boolean = settle(ticket, setOf(State.Reserved), State.CancelledBeforeBirth)

    /** Close-before-start consumes the external identity and cancels an unstarted reservation. */
    fun retireOwner(kind: Kind, logicalId: String) = synchronized(lock) {
        require(kind in OWNER_KINDS && logicalId.isNotBlank())
        val key = kind to logicalId
        if (!usedOwners.containsKey(key)) {
            usedOwners[key] = null
            revision++
        }
        val ticket = usedOwners[key]
        if (ticket != null) {
            val current = entries[ticket.id]
            if (current?.state == State.Reserved) {
                entries[ticket.id] = current.copy(state = State.CancelledBeforeBirth)
                revision++
            }
        }
    }

    fun state(ticket: Ticket): State? = synchronized(lock) {
        entries[ticket.id]?.takeIf { it.ticket == ticket }?.state
    }

    /** Only an exact native close that returned successfully may call this. */
    fun closedExact(ticket: Ticket): Boolean = settle(ticket, setOf(State.BirthEntered), State.ClosedExact)

    /** A control operation returns only after its own work has left native code. */
    fun completeOperation(ticket: Ticket): Boolean = settle(ticket, setOf(State.Reserved, State.BirthEntered), State.Completed)

    fun unknown(ticket: Ticket): Boolean = settle(ticket, setOf(State.Reserved, State.BirthEntered), State.Unknown)

    /** Go checkConfig currently ignores box.Close's result; a captured call cannot prove cleanup. */
    fun validationCleanupUnknown(ticket: Ticket): Boolean =
        settle(ticket, setOf(State.BirthEntered), State.ValidationCleanupUnknown)

    private fun settle(ticket: Ticket, from: Set<State>, to: State): Boolean = synchronized(lock) {
        val current = entries[ticket.id] ?: return@synchronized false
        if (current.ticket != ticket || current.state !in from) return@synchronized false
        entries[ticket.id] = current.copy(state = to)
        revision++
        true
    }

    /** Sealing is permanent. Repeating the same fence is idempotent; a second fence is rejected. */
    fun seal(id: String): AndroidDrainReceipt = synchronized(lock) {
        require(id.isNotBlank() && id == id.trim() && id.length <= 128) { "invalid native fence ID" }
        val existing = fenceId
        check(existing == null || existing == id) { "a different native fence already sealed this process" }
        if (existing == null) {
            fenceId = id
            captured = entries.values.filter {
                it.state !in TERMINAL || it.state == State.Unknown || it.state == State.ValidationCleanupUnknown
            }.map { it.ticket.id }
            revision++
            sealedRevision = revision
        }
        receiptLocked(id)
    }

    fun receipt(id: String): AndroidDrainReceipt = synchronized(lock) {
        check(fenceId == id) { "native fence was not sealed with this ID" }
        receiptLocked(id)
    }

    private fun receiptLocked(id: String) = AndroidDrainReceipt(
        processNonce, id, bootstrap, sealedRevision, revision,
        captured.map { ticket -> checkNotNull(entries[ticket]) },
    )

    companion object {
        private val OWNER_KINDS = setOf(Kind.Main, Kind.Login, Kind.Speedtest)
        private val TERMINAL = setOf(State.CancelledBeforeBirth, State.ClosedExact, State.Completed,
            State.Unknown, State.ValidationCleanupUnknown)
    }
}

/** One ledger for the entire Android process, including cold always-on Service starts. */
internal object AndroidNativeAdmissionGate {
    private const val REQUIRED_MARKER = "mesh-route-state.required"
    val ledger = AndroidNativeAdmission()

    fun bootstrap(context: Context) {
        val marker = File(File(context.dataDir, "polaris"), REQUIRED_MARKER)
        val proof = try {
            Os.stat(marker.path)
            RequiredMarkerProof.PresentOrUnknown
        } catch (error: ErrnoException) {
            if (error.errno == OsConstants.ENOENT) RequiredMarkerProof.Absent
            else RequiredMarkerProof.PresentOrUnknown
        } catch (_: Throwable) {
            RequiredMarkerProof.PresentOrUnknown
        }
        ledger.bootstrap(proof)
    }
}
