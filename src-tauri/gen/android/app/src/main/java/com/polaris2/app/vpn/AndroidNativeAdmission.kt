package com.polaris2.app.vpn

import android.content.Context
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.util.UUID

/** The result of one cold-process stat. Only ENOENT opens native admission. */
internal enum class RequiredMarkerProof { Absent, PresentOrUnknown }

internal enum class AndroidNativeProducer(val wireName: String) {
    MainBridge("main.bridge"), MainSystem("main.system"), MainClose("main.close"),
    MainReload("main.reload"), LoginStart("login.start"), LoginClose("login.close"),
    SpeedtestStart("speedtest.start"), SpeedtestClose("speedtest.close"),
    ValidationCheckConfig("validation.checkConfig"),
    TargetlessStop("control.targetlessStop"), TargetlessReload("control.targetlessReload"),
}

/** Build-time wiring manifest. A producer is listed only after all its entry and close paths are tested. */
internal object AndroidNativeCoverage {
    const val PROTOCOL_VERSION = 1
    val requiredProducers = AndroidNativeProducer.entries.map(AndroidNativeProducer::wireName).toSet()
    // Only installed adapters can declare capabilities. Incomplete families keep
    // the verifier closed even when the captured ticket list happens to be empty.
    val wiredProducers: Set<String> get() =
        (AndroidNativeMain.capabilities + AndroidNativeValidation.capabilities + TransientSpeedtestSessions.capabilities +
            TransientLoginNativeOwner.capabilities)
            .map(AndroidNativeProducer::wireName).toSet()
}

/** Frozen membership plus live terminal facts; no field on its own asserts NoOldCore. */
internal data class AndroidDrainReceipt(
    val protocolVersion: Int,
    val coveredProducers: List<String>,
    val coverageComplete: Boolean,
    val processNonce: String,
    val fenceId: String,
    val markerProof: RequiredMarkerProof?,
    val sealedRevision: Long,
    val revision: Long,
    val capturedCount: Int,
    val captured: List<AndroidNativeAdmission.Entry>,
)

/** Pure process ledger. Its monitor protects only state; callers do all I/O and JNI outside it. */
internal class AndroidNativeAdmission(
    val processNonce: String = UUID.randomUUID().toString(),
    private val coveredProducers: Set<String> = AndroidNativeCoverage.wiredProducers,
    private val maxMetadataRecords: Int = DEFAULT_MAX_METADATA_RECORDS,
) {
    init {
        require(validId(processNonce, 128)) { "invalid native process nonce" }
        require(AndroidNativeCoverage.requiredProducers.containsAll(coveredProducers)) { "invalid native coverage manifest" }
        require(maxMetadataRecords in 1..DEFAULT_MAX_METADATA_RECORDS) { "invalid native metadata budget" }
    }
    enum class Kind { Main, Login, Speedtest, CheckConfig, TargetlessStop, TargetlessReload }
    enum class State { Reserved, BirthEntered, CancelledBeforeBirth, ClosedExact, Completed, Unknown, ValidationCleanupUnknown }
    data class Ticket(val id: String, val kind: Kind, val logicalId: String)
    data class Entry(val ticket: Ticket, val state: State)
    open class AdmissionClosed(message: String = "android: native admission is closed") : IllegalStateException(message)
    class CapacityClosed : AdmissionClosed(CAPACITY_MESSAGE)
    data class MetadataUsage(val entries: Int, val consumedOwners: Int, val limit: Int, val capacityClosed: Boolean) {
        val records: Int get() = entries + consumedOwners
    }

    private val lock = Any()
    private var bootstrap: RequiredMarkerProof? = null
    private var fenceId: String? = null
    private var sealedRevision = 0L
    private var revision = 0L
    private var capacityClosed = false
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

    private fun reserve(kind: Kind, logicalId: String, owner: Boolean): Ticket {
        val ticket = Ticket(UUID.randomUUID().toString(), kind, logicalId)
        return synchronized(lock) {
            if (capacityClosed) throw CapacityClosed()
            if (fenceId != null || bootstrap != RequiredMarkerProof.Absent) throw AdmissionClosed()
            require(validId(logicalId, 256)) { "invalid native owner identity" }
            check(!entries.containsKey(ticket.id)) { "native ticket collision" }
            if (owner) check(!usedOwners.containsKey(kind to logicalId)) { "native owner identity was already consumed" }
            requireCapacityLocked(if (owner) 2 else 1)
            if (owner) usedOwners[kind to logicalId] = ticket
            entries[ticket.id] = Entry(ticket, State.Reserved)
            revision++
            closeIfFullLocked()
            ticket
        }
    }

    /** A worker must call this immediately before crossing a native factory/JNI boundary. */
    fun enterBirth(ticket: Ticket): Boolean = synchronized(lock) {
        val current = entries[ticket.id] ?: return@synchronized false
        if (!birthAllowedLocked(ticket)) return@synchronized false
        entries[ticket.id] = current.copy(state = State.BirthEntered)
        revision++
        true
    }

    /** A preflight before disrupting another owner; enterBirth still rechecks at the native boundary. */
    fun birthAllowed(ticket: Ticket): Boolean = synchronized(lock) { birthAllowedLocked(ticket) }
    private fun birthAllowedLocked(ticket: Ticket): Boolean = entries[ticket.id]?.let {
        it.ticket == ticket && it.state == State.Reserved && fenceId == null && !capacityClosed
    } == true

    /** A reservation cancelled before native birth is a positive terminal fact. */
    fun cancelBeforeBirth(ticket: Ticket): Boolean = settle(ticket, setOf(State.Reserved), State.CancelledBeforeBirth)

    /** Close-before-start consumes the external identity and cancels an unstarted reservation. */
    fun retireOwner(kind: Kind, logicalId: String) = synchronized(lock) {
        require(kind in OWNER_KINDS && validId(logicalId, 256))
        val key = kind to logicalId
        if (!usedOwners.containsKey(key)) {
            requireCapacityLocked(1)
            usedOwners[key] = null
            revision++
            closeIfFullLocked()
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

    fun metadataUsage(): MetadataUsage = synchronized(lock) {
        MetadataUsage(entries.size, usedOwners.size, maxMetadataRecords, capacityClosed)
    }

    fun admissionRejection(): AdmissionClosed = synchronized(lock) {
        if (capacityClosed) CapacityClosed() else AdmissionClosed()
    }

    /** Capacity is a permanent admission failure, never a drain fence or terminal resource fact. */
    private fun requireCapacityLocked(additional: Int) {
        if (capacityClosed || additional > maxMetadataRecords - entries.size - usedOwners.size) {
            closeCapacityLocked()
            throw CapacityClosed()
        }
    }

    private fun closeIfFullLocked() {
        if (entries.size + usedOwners.size == maxMetadataRecords) closeCapacityLocked()
    }

    private fun closeCapacityLocked() {
        if (!capacityClosed) { capacityClosed = true; revision++ }
    }

    /** Only an exact native close that returned successfully may call this. */
    fun closedExact(ticket: Ticket): Boolean {
        require(ticket.kind in OWNER_KINDS)
        return settle(ticket, setOf(State.BirthEntered), State.ClosedExact)
    }

    /** A control operation returns only after its own work has left native code. */
    fun completeOperation(ticket: Ticket): Boolean {
        require(ticket.kind in CONTROL_KINDS)
        return settle(ticket, setOf(State.Reserved, State.BirthEntered), State.Completed)
    }

    fun unknown(ticket: Ticket): Boolean = settle(ticket, setOf(State.Reserved, State.BirthEntered), State.Unknown)

    /** Android's legacy checkConfig call does not consume the typed Go disposal contract. */
    fun validationCleanupUnknown(ticket: Ticket): Boolean {
        require(ticket.kind == Kind.CheckConfig)
        return settle(ticket, setOf(State.BirthEntered), State.ValidationCleanupUnknown)
    }

    private fun settle(ticket: Ticket, from: Set<State>, to: State): Boolean = synchronized(lock) {
        val current = entries[ticket.id] ?: return@synchronized false
        if (current.ticket != ticket || current.state !in from) return@synchronized false
        entries[ticket.id] = current.copy(state = to)
        revision++
        true
    }

    /** Sealing is permanent. Repeating the same fence is idempotent; a second fence is rejected. */
    fun seal(id: String): AndroidDrainReceipt = synchronized(lock) {
        require(validId(id, 128)) { "invalid native fence ID" }
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
        AndroidNativeCoverage.PROTOCOL_VERSION,
        coveredProducers.sorted(),
        coveredProducers == AndroidNativeCoverage.requiredProducers,
        processNonce, id, bootstrap, sealedRevision, revision,
        captured.size,
        captured.map { ticket -> checkNotNull(entries[ticket]) },
    )

    companion object {
        // A record budget, not a byte estimate. IDs are bounded; each owner uses
        // two records, each validation/control one, and an unseen close one.
        const val DEFAULT_MAX_METADATA_RECORDS = 16_384
        const val CAPACITY_CODE = "ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED"
        const val CAPACITY_MESSAGE = "本次运行的生命周期记录已满，请完全关闭并重新启动应用后重试。"
        // Every producer uses UUID, hex, or a sanitized ASCII file stem. An explicit
        // shared alphabet avoids JVM/Rust Unicode whitespace and surrogate differences.
        private fun validId(value: String, maxUtf16Units: Int): Boolean =
            value.isNotEmpty() && value.length <= maxUtf16Units && value.all {
                it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it in "._:-"
            }
        private val OWNER_KINDS = setOf(Kind.Main, Kind.Login, Kind.Speedtest)
        private val CONTROL_KINDS = setOf(Kind.TargetlessStop, Kind.TargetlessReload)
        private val TERMINAL = setOf(State.CancelledBeforeBirth, State.ClosedExact, State.Completed,
            State.Unknown, State.ValidationCleanupUnknown)
    }
}

/** An explicit rejection cause for internal callbacks; arbitrary native text carries no code. */
internal data class AndroidNativeFailure(val message: String, val code: String? = null) {
    companion object {
        fun from(error: Throwable, fallback: String): AndroidNativeFailure =
            if (error is AndroidNativeAdmission.CapacityClosed) capacity() else AndroidNativeFailure(fallback)
        fun capacity() = AndroidNativeFailure(AndroidNativeAdmission.CAPACITY_MESSAGE, AndroidNativeAdmission.CAPACITY_CODE)
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
