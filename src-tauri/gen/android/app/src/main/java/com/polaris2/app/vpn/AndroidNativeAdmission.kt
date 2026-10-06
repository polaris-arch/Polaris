package com.polaris2.app.vpn

import android.content.Context
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.util.UUID
import java.security.MessageDigest
import org.json.JSONArray
import org.json.JSONObject

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
    data class Entry(val ticket: Ticket, val state: State,
        internal val store: AndroidTailscaleStoreCustody? = null,
        internal val validation: AndroidTailscaleValidationScope? = null,
        internal val parent: Ticket? = null,
        internal val targetAction: Pair<String, String>? = null,
        internal val scopedCloseRequest: String? = null,
        internal val scopedBytes: Int = 0,
        internal val warm: Warm? = null)
    internal class Warm(val tuple: JSONObject, val bytes: Int) {
        var claimed = false
        var registrationOpen = false
        var configBound = false
        var child: Ticket? = null
        var finished = false
    }
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
        synchronized(lock) {
            val original = usedOwners[kind to logicalId]?.let { entries[it.id] }
            val warm = original?.warm
            if (warm != null && kind == Kind.Login) {
                check(!warm.finished && !warm.claimed && original.state == State.Reserved &&
                    original.targetAction != null && fenceId == null && !capacityClosed) { "nativeRetirementUnknown" }
                warm.claimed = true
                warm.registrationOpen = true
                revision++
                return original.ticket
            }
        }
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
        it.ticket == ticket && it.state == State.Reserved && fenceId == null && !capacityClosed &&
            (entries.values.none { entry -> entry.targetAction != null } || warmBirthAllowedLocked(ticket))
    } == true

    private fun warmBirthAllowedLocked(ticket: Ticket): Boolean {
        val owner = entries.values.firstOrNull { it.warm != null && (it.ticket == ticket || it.warm.child == ticket) } ?: return false
        val warm = checkNotNull(owner.warm)
        if (owner.targetAction == null || warm.finished || !warm.claimed || warm.registrationOpen || !warm.configBound) return false
        return if (owner.ticket == ticket) warm.child?.let { entries[it.id]?.state == State.Reserved } == true
            else owner.state == State.BirthEntered && warm.child == ticket
    }

    fun isWarmLogin(ticket: Ticket): Boolean = synchronized(lock) { entries[ticket.id]?.takeIf { it.ticket == ticket }?.warm != null }

    fun bindWarmConfig(ticket: Ticket, config: String) {
        val digest = AndroidTailscaleStoreCustody.digest(config)
        val noAuth = AndroidTailscaleStoreCustody.noAuthConfig(config)
        synchronized(lock) {
            val owner = checkNotNull(entries[ticket.id]); val warm = owner.warm ?: return
            check(owner.ticket == ticket && owner.state == State.Reserved && warm.claimed && !warm.finished)
            check(noAuth && digest == warm.tuple.getJSONObject("validation").getJSONObject("validation").getString("configDigest")) { "nativeRetirementUnknown" }
            warm.configBound = true
        }
    }

    fun reserveWarmValidation(ticket: Ticket): Ticket = synchronized(lock) {
        val owner = checkNotNull(entries[ticket.id]); val warm = checkNotNull(owner.warm)
        check(owner.ticket == ticket && owner.state == State.Reserved && owner.targetAction != null &&
            warm.claimed && warm.configBound && warm.registrationOpen && warm.child == null && !warm.finished)
        val child = reserveOperation(Kind.CheckConfig)
        warm.child = child
        warm.registrationOpen = false
        revision++
        child
    }

    /** Original synchronous preparation has finished reserving (or failed before reserving) its child. */
    fun finishWarmPreparation(ticket: Ticket) = synchronized(lock) {
        entries[ticket.id]?.takeIf { it.ticket == ticket }?.warm?.let { it.registrationOpen = false; revision++ }
    }

    fun checkWarmValidationConfig(ticket: Ticket, config: String) {
        val digest = AndroidTailscaleStoreCustody.digest(config)
        synchronized(lock) {
            val warm = entries.values.firstOrNull { it.warm?.child == ticket }?.warm ?: return
            check(digest == warm.tuple.getJSONObject("validation").getJSONObject("validation").getString("configDigest")) { "nativeRetirementUnknown" }
        }
    }

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

    /** Private attachment to the original Entry; never changes global state/capabilities. */
    fun bindTailscaleStore(ticket: Ticket, birthNonce: String = ""): AndroidTailscaleStoreCustody = synchronized(lock) {
        val entry = checkNotNull(entries[ticket.id])
        check(entry.ticket == ticket && ticket.kind in OWNER_KINDS)
        entry.store?.also { check(it.birthNonce == birthNonce) } ?: AndroidTailscaleStoreCustody(this, ticket, birthNonce).also {
            entries[ticket.id] = entry.copy(store = it)
            revision++
        }
    }

    internal fun scopedChanged(ticket: Ticket) = synchronized(lock) {
        check(entries[ticket.id]?.ticket == ticket)
        revision++
    }

    /** A bounded attachment may stay Unknown; existing owner/census records are never evicted. */
    internal fun admitScopedBytes(ticket: Ticket, bytes: Int): Boolean = synchronized(lock) {
        val entry = checkNotNull(entries[ticket.id])
        check(entry.ticket == ticket)
        val total = bytes.toLong() + (entry.warm?.bytes ?: 0)
        if (bytes < 0 || total > MAX_SCOPED_BYTES - entries.values.sumOf { it.scopedBytes } + entry.scopedBytes) return@synchronized false
        entries[ticket.id] = entry.copy(scopedBytes = total.toInt())
        true
    }

    fun attachValidation(ticket: Ticket, scope: AndroidTailscaleValidationScope) = synchronized(lock) {
        val entry = checkNotNull(entries[ticket.id])
        check(entry.ticket == ticket && ticket.kind == Kind.CheckConfig && entry.state == State.BirthEntered)
        check(entry.validation == null)
        val bytes = scope.wire().toString().toByteArray(Charsets.UTF_8).size
        if (bytes <= AndroidTailscaleStoreCustody.MAX_BYTES && admitScopedBytes(ticket, bytes)) entries[ticket.id] = checkNotNull(entries[ticket.id]).copy(validation = scope)
        revision++
    }

    fun linkControl(ticket: Ticket, parent: Ticket?) = synchronized(lock) {
        val entry = checkNotNull(entries[ticket.id])
        check(entry.ticket == ticket && ticket.kind in CONTROL_KINDS && entry.state == State.Reserved)
        val original = parent?.takeIf { it.kind == Kind.Main && entries[it.id]?.ticket == it }
        entries[ticket.id] = entry.copy(parent = original)
        revision++
    }

    fun observeTailscaleOwner(kind: Kind, logicalId: String): JSONObject? = synchronized(lock) {
        usedOwners[kind to logicalId]?.let { entries[it.id]?.store?.wire() }
    }

    fun readTailscaleOwner(binding: JSONObject): JSONObject? = synchronized(lock) {
        originalStoreLocked(binding)?.wire()
    }

    fun beginTailscaleClose(binding: JSONObject, requestId: String) = synchronized(lock) {
        require(validId(requestId, 128))
        val store = originalStoreLocked(binding) ?: throw AdmissionClosed("nativeRetirementUnknown")
        val entry = checkNotNull(entries[store.ticket.id])
        check(entry.scopedCloseRequest == null || entry.scopedCloseRequest == requestId) { "nativeRetirementUnknown" }
        entries[store.ticket.id] = entry.copy(scopedCloseRequest = requestId)
        revision++
    }

    private fun originalStoreLocked(binding: JSONObject): AndroidTailscaleStoreCustody? {
        if (binding.optString("processNonce") != processNonce) return null
        val store = entries[binding.optString("nativeTicketId")]?.store ?: return null
        return store.takeIf { it.matches(binding) }
    }

    /** No JNI/FS/await while held. The reservation itself remains in this exact Entry. */
    fun tailscaleTargetAction(binding: JSONObject, stateFile: String, actionId: String, operation: String): JSONObject = synchronized(lock) {
        require(validId(actionId, 128))
        val store = originalStoreLocked(binding) ?: throw AdmissionClosed("nativeRetirementUnknown")
        check(store.containsOriginalFile(stateFile)) { "nativeRetirementUnknown" }
        val owner = checkNotNull(entries[store.ticket.id])
        check(owner.warm == null) { "nativeRetirementUnknown" } // Warm has its own exact terminal/revocation path.
        if (operation == "finish") {
            check(owner.targetAction == (actionId to stateFile)) { "nativeRetirementUnknown" }
            entries[store.ticket.id] = owner.copy(targetAction = null)
            revision++
            return@synchronized JSONObject().put("released", true).put("processNonce", processNonce)
                .put("nativeTicketId", store.ticket.id).put("actionRequestId", actionId).put("stateFile", stateFile)
        }
        check(operation == "query" || operation == "begin")
        check(entries.values.none { it.targetAction != null }) { "nativeRetirementUnknown" }
        val rows = targetRowsLocked(stateFile)
        val result = JSONObject().put("processNonce", processNonce).put("revision", revision)
            .put("actionRequestId", actionId).put("stateFile", stateFile).put("entries", rows)
        check(result.toString().toByteArray(Charsets.UTF_8).size <= AndroidTailscaleStoreCustody.MAX_BYTES) { "nativeRetirementUnknown" }
        if (operation == "begin") {
            entries[store.ticket.id] = owner.copy(targetAction = actionId to stateFile)
            revision++
            result.put("revision", revision)
        }
        result
    }

    private fun targetRowsLocked(stateFile: String): JSONArray {
        val rows = JSONArray()
        var rowBytes = 0
        for (entry in entries.values) {
            val related = when {
                entry.state == State.CancelledBeforeBirth -> false
                entry.ticket.kind == Kind.CheckConfig -> if (entry.state == State.ValidationCleanupUnknown) entry.validation?.related(stateFile) else null
                entry.ticket.kind in CONTROL_KINDS -> if (entry.state == State.Reserved || entry.state == State.BirthEntered) null
                    else entry.parent?.let { entries[it.id]?.store?.closedRelation(stateFile) }
                else -> entry.store?.closedRelation(stateFile)
            }
            check(related != null) { "nativeRetirementUnknown" }
            val row = JSONObject().put("nativeTicketId", entry.ticket.id).put("producerKind", entry.ticket.kind.name)
                .put("logicalInstanceId", entry.ticket.logicalId).put("globalState", entry.state.name).put("related", related)
            entry.store?.let { row.put("store", it.wire()) }
            entry.validation?.let { row.put("validation", it.wire()) }
            if (entry.state != State.CancelledBeforeBirth) entry.parent?.let { row.put("parentNativeTicketId", it.id) }
            rowBytes += row.toString().toByteArray(Charsets.UTF_8).size
            check(rowBytes <= AndroidTailscaleStoreCustody.MAX_BYTES) { "nativeRetirementUnknown" }
            rows.put(row)
        }
        return rows
    }

    fun validationOwner(ticket: Ticket): JSONObject? = synchronized(lock) {
        val original = entries[ticket.id]?.takeIf { it.ticket == ticket && ticket.kind == Kind.CheckConfig && it.state == State.ValidationCleanupUnknown } ?: return@synchronized null
        original.validation?.let { scope -> JSONObject().put("contractVersion", "polaris-android-validation-custody-v1")
            .put("processNonce", processNonce).put("nativeTicketId", ticket.id).put("validation", scope.wire()) }
    }

    private fun validationBindingLocked(binding: JSONObject): Entry {
        check(binding.keys().asSequence().toSet() == setOf("contractVersion", "processNonce", "nativeTicketId", "validation"))
        check(binding.getString("contractVersion") == "polaris-android-validation-custody-v1" && binding.getString("processNonce") == processNonce)
        val original = checkNotNull(entries[binding.getString("nativeTicketId")])
        check(original.ticket.kind == Kind.CheckConfig && original.state == State.ValidationCleanupUnknown)
        check(checkNotNull(original.validation).wire().similar(binding.getJSONObject("validation")))
        return original
    }

    private fun warmOwnerLocked(tuple: JSONObject): Entry {
        val binding = tuple.getJSONObject("validation")
        validationBindingLocked(binding)
        val ticket = checkNotNull(usedOwners[Kind.Login to tuple.getString("logicalInstanceId")])
        val owner = checkNotNull(entries[ticket.id]); val warm = checkNotNull(owner.warm)
        check(warm.tuple.similar(tuple) && owner.ticket == ticket)
        return owner
    }

    fun warmTicket(tuple: JSONObject): Ticket = synchronized(lock) { warmOwnerLocked(tuple).ticket }

    fun beginWarm(tuple: JSONObject): JSONObject = synchronized(lock) {
        check(tuple.keys().asSequence().toSet() == setOf("contractVersion", "validation", "stateFile", "actionRequestId", "logicalInstanceId"))
        check(tuple.getString("contractVersion") == "polaris-android-ts-cold-warm-v1")
        val id = tuple.getString("logicalInstanceId"); val action = tuple.getString("actionRequestId"); val file = tuple.getString("stateFile")
        require(validId(id, 256) && validId(action, 128))
        if (usedOwners.containsKey(Kind.Login to id)) return@synchronized warmWireLocked(warmOwnerLocked(tuple))
        val original = validationBindingLocked(tuple.getJSONObject("validation"))
        check(checkNotNull(original.validation).selectsWarm(file)) { "nativeRetirementUnknown" }
        check(entries.values.none { it.targetAction != null })
        val rows = targetRowsLocked(file)
        check(rows.toString().toByteArray(Charsets.UTF_8).size <= AndroidTailscaleStoreCustody.MAX_BYTES)
        val bytes = tuple.toString().toByteArray(Charsets.UTF_8).size
        check(bytes <= AndroidTailscaleStoreCustody.MAX_BYTES && bytes <= MAX_SCOPED_BYTES - entries.values.sumOf { it.scopedBytes })
        requireCapacityLocked(3)
        val ticket = reserve(Kind.Login, id, owner = true)
        val warm = Warm(JSONObject(tuple.toString()), bytes)
        entries[ticket.id] = checkNotNull(entries[ticket.id]).copy(warm = warm, targetAction = action to file, scopedBytes = bytes)
        revision++
        warmWireLocked(checkNotNull(entries[ticket.id]))
    }

    fun beginWarmClose(tuple: JSONObject, requestId: String) = synchronized(lock) {
        require(validId(requestId, 128))
        val owner = warmOwnerLocked(tuple)
        check(owner.scopedCloseRequest == null || owner.scopedCloseRequest == requestId)
        entries[owner.ticket.id] = owner.copy(scopedCloseRequest = requestId)
        revision++
    }

    fun readWarm(tuple: JSONObject): JSONObject = synchronized(lock) { warmWireLocked(warmOwnerLocked(tuple)) }

    private fun warmWireLocked(owner: Entry): JSONObject {
        val warm = checkNotNull(owner.warm)
        val file = warm.tuple.getString("stateFile"); val action = warm.tuple.getString("actionRequestId")
        val held = owner.targetAction == (action to file) && !warm.finished
        val child = warm.child?.let { entries[it.id] }
        val result = JSONObject().put("tuple", JSONObject(warm.tuple.toString())).put("nativeTicketId", owner.ticket.id).put("globalState", owner.state.name)
            .put("held", held).put("claimed", warm.claimed).put("childRegistrationOpen", warm.registrationOpen)
            .put("childNativeTicketId", child?.ticket?.id ?: "").put("childGlobalState", child?.state?.name ?: "")
        owner.store?.let { store ->
            val runtime = store.wire()
            if (result.toString().toByteArray(Charsets.UTF_8).size + runtime.toString().toByteArray(Charsets.UTF_8).size < AndroidTailscaleStoreCustody.MAX_BYTES) result.put("runtime", runtime)
        }
        if (held && owner.store?.closedRelation(file) == true) runCatching {
            val family = JSONObject().put("processNonce", processNonce).put("revision", revision).put("actionRequestId", action)
                .put("stateFile", file).put("entries", targetRowsLocked(file))
            check(result.toString().toByteArray(Charsets.UTF_8).size + family.toString().toByteArray(Charsets.UTF_8).size < AndroidTailscaleStoreCustody.MAX_BYTES)
            result.put("family", family)
        }
        check(result.toString().toByteArray(Charsets.UTF_8).size <= AndroidTailscaleStoreCustody.MAX_BYTES)
        return result
    }

    fun finishWarm(tuple: JSONObject): JSONObject = synchronized(lock) {
        check(tuple.keys().asSequence().toSet() == setOf("contractVersion", "validation", "stateFile", "actionRequestId", "logicalInstanceId"))
        check(tuple.getString("contractVersion") == "polaris-android-ts-cold-warm-v1")
        validationBindingLocked(tuple.getJSONObject("validation"))
        val action = tuple.getString("actionRequestId"); val file = tuple.getString("stateFile"); val id = tuple.getString("logicalInstanceId")
        require(validId(action, 128) && validId(id, 256))
        if (usedOwners[Kind.Login to id] == null) {
            // A cancelled original intent consumes this ID before a late begin can reserve it.
            // At permanent capacity the original birth gate is already irreversibly shut.
            if (!usedOwners.containsKey(Kind.Login to id) && !capacityClosed) retireOwner(Kind.Login, id)
            return@synchronized JSONObject().put("released", true).put("processNonce", processNonce).put("nativeTicketId", "")
                .put("actionRequestId", action).put("stateFile", file).put("beforeBirth", true)
        }
        val owner = warmOwnerLocked(tuple); val warm = checkNotNull(owner.warm)
        if (!warm.finished) {
            check(owner.targetAction == (action to file))
            val child = warm.child?.let { checkNotNull(entries[it.id]) }
            val beforeBirth = owner.state in setOf(State.Reserved, State.CancelledBeforeBirth) && !warm.registrationOpen &&
                (child == null || child.state in setOf(State.Reserved, State.CancelledBeforeBirth))
            if (beforeBirth) {
                cancelBeforeBirth(owner.ticket)
                child?.let { cancelBeforeBirth(it.ticket) }
            } else {
                check(owner.store?.closedRelation(file) == true && child != null &&
                    child.state in setOf(State.CancelledBeforeBirth, State.ValidationCleanupUnknown)) { "nativeRetirementUnknown" }
                targetRowsLocked(file) // All original related writers; no ordinary Close substitution.
            }
            warm.finished = true
            entries[owner.ticket.id] = checkNotNull(entries[owner.ticket.id]).copy(targetAction = null)
            revision++
        }
        JSONObject().put("released", true).put("processNonce", processNonce).put("nativeTicketId", owner.ticket.id)
            .put("actionRequestId", action).put("stateFile", file).put("beforeBirth", false)
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
        internal const val MAX_SCOPED_BYTES = 8 * AndroidTailscaleStoreCustody.MAX_BYTES
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

/** Private native-call history in its original Entry, not a second owner registry. */
internal class AndroidTailscaleStoreCustody internal constructor(
    private val ledger: AndroidNativeAdmission,
    val ticket: AndroidNativeAdmission.Ticket,
    val birthNonce: String,
) {
    internal data class Scope(val tag: String, val directory: String, val file: String) {
        fun json() = JSONObject().put("tag", tag).put("stateDirectory", directory).put("stateFile", file)
    }
    internal data class Run(val nonce: String, val digest: String, val scopes: List<Scope>) {
        fun json() = JSONObject().put("runNonce", nonce).put("configDigest", digest)
            .put("scopes", JSONArray(scopes.map { it.json() }))
    }
    private var runs = emptyList<Run>()
    private var currentDigest = ""
    private var payload = ""
    private var unknown = false
    private var terminal = false
    private var nativeTerminalComplete = false
    private var nativeCensusComplete = false

    /** Captures both sides of this exact call even when JNI throws. No capture error replaces it. */
    fun <T> invoke(config: String, export: () -> String, action: () -> T): T {
        val digest = digest(config)
        val before = runCatching { parseRuns(export()) }.getOrNull()
        synchronized(this) {
            if (before == null || !sameRuns(before, runs) || terminal) unknown = true
        }
        var returned = false
        try { return action().also { returned = true } }
        finally {
            val raw = runCatching(export).getOrNull()
            val after = raw?.let { runCatching { parseRuns(it) }.getOrNull() }
            val projected = synchronized(this) {
                val fresh = after?.filter { next -> runs.none { it.nonce == next.nonce } }.orEmpty()
                if (after != null && fresh.size == 1 && fresh[0].digest == digest && runs.all { it in after }) runs + fresh else runs
            }
            val admitted = raw != null && raw.toByteArray(Charsets.UTF_8).size <= MAX_BYTES &&
                snapshotBytes(raw, projected, if (returned) digest else synchronized(this) { currentDigest }, false)?.let { ledger.admitScopedBytes(ticket, it) } == true
            synchronized(this) {
                val old = runs.associateBy { it.nonce }
                val fresh = after?.filter { !old.containsKey(it.nonce) }.orEmpty()
                if (!returned || before == null || after == null || fresh.size != 1 ||
                    fresh.any { it.digest != digest } ||
                    old.values.any { original -> after.none { it == original } }) unknown = true
                if (after != null && fresh.size == 1 && fresh[0].digest == digest &&
                    old.values.all { original -> after.any { it == original } } && admitted) runs = runs + fresh
                if (returned) currentDigest = digest
                if (admitted) payload = checkNotNull(raw)
                else unknown = true
            }
            ledger.scopedChanged(ticket)
        }
    }

    /** Call after the original closeService, before closeServer/handle clear and after pending Start joins. */
    fun closed(export: () -> String) {
        val raw = runCatching(export).getOrNull()
        val parsed = raw?.let { runCatching { parseRuns(it) }.getOrNull() }
        val nativeComplete = raw?.let { runCatching { terminalComplete(it) }.getOrDefault(false) } == true
        val censusComplete = raw?.let { runCatching { censusComplete(it) }.getOrDefault(false) } == true
        val originalRuns = synchronized(this) { runs }
        val admitted = raw != null && raw.toByteArray(Charsets.UTF_8).size <= MAX_BYTES &&
            snapshotBytes(raw, originalRuns, synchronized(this) { currentDigest }, true)?.let { ledger.admitScopedBytes(ticket, it) } == true
        synchronized(this) {
            if (parsed == null || runs.isEmpty() || !sameRuns(parsed, runs)) unknown = true
            if (admitted) payload = checkNotNull(raw)
            else unknown = true
            terminal = true
            nativeTerminalComplete = nativeComplete
            nativeCensusComplete = censusComplete
        }
        ledger.scopedChanged(ticket)
    }

    @Synchronized fun wire(): JSONObject = wireValue(runs, currentDigest, payload, unknown, terminal)

    private fun wireValue(originalRuns: List<Run>, digest: String, raw: String, historyUnknown: Boolean, closed: Boolean): JSONObject = JSONObject()
        .put("contractVersion", "polaris-android-ts-store-custody-v1")
        .put("producerKind", ticket.kind.name).put("processNonce", ledger.processNonce)
        .put("nativeTicketId", ticket.id).put("logicalInstanceId", ticket.logicalId)
        .put("mainBirthNonce", birthNonce).put("actualConfigDigest", digest)
        .put("originalObservedRuns", JSONArray(originalRuns.map { it.json() }))
        .put("historyUnknown", historyUnknown).put("terminal", closed).put("storeRetirement", raw)

    private fun snapshotBytes(raw: String, originalRuns: List<Run>, digest: String, closed: Boolean): Int? {
        val historyBytes = JSONArray(originalRuns.map { it.json() }).toString().toByteArray(Charsets.UTF_8).size
        // Check both parts before composing an envelope; JSON escaping is included in the exact final bound.
        if (historyBytes > MAX_BYTES || raw.toByteArray(Charsets.UTF_8).size > MAX_BYTES - historyBytes) return null
        val bytes = wireValue(originalRuns, digest, raw, false, closed).toString().toByteArray(Charsets.UTF_8).size
        return bytes.takeIf { it <= MAX_BYTES }
    }

    @Synchronized fun matches(binding: JSONObject): Boolean = runCatching {
        binding.getString("contractVersion") == "polaris-android-ts-store-custody-v1" &&
            binding.getString("processNonce") == ledger.processNonce && binding.getString("producerKind") == ticket.kind.name &&
            binding.getString("nativeTicketId") == ticket.id && binding.getString("logicalInstanceId") == ticket.logicalId &&
            binding.getString("mainBirthNonce") == birthNonce && binding.getString("actualConfigDigest") == currentDigest &&
            sameRuns(observedRuns(binding.getJSONArray("originalObservedRuns")), runs)
    }.getOrDefault(false)

    @Synchronized fun containsOriginalFile(file: String): Boolean = runs.any { run -> run.scopes.any { it.file == file } }

    @Synchronized fun closedRelation(file: String): Boolean? {
        if (unknown || !terminal || runs.isEmpty() || !hex64(currentDigest)) return null
        if (!nativeCensusComplete) return null
        if (!containsOriginalFile(file)) return false
        return if (nativeTerminalComplete) true else null
    }

    companion object {
        const val MAX_BYTES = 1_048_576
        internal val KNOWN_WRITERS = setOf("SealedDrained", "NoStoreConstruction")
        fun digest(config: String): String = MessageDigest.getInstance("SHA-256")
            .digest(config.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) }
        fun hex64(value: String): Boolean = value.matches(Regex("[0-9a-f]{64}"))
        private fun sameRuns(left: List<Run>, right: List<Run>): Boolean =
            left.size == right.size && left.associateBy { it.nonce } == right.associateBy { it.nonce }
        private fun scopes(array: JSONArray): List<Scope> {
            check(array.length() <= 512)
            val result = (0 until array.length()).map { index ->
                val value = array.getJSONObject(index)
                Scope(value.getString("tag"), value.getString("stateDirectory"), value.getString("stateFile"))
            }
            check(result.all { it.tag.isNotEmpty() && it.directory.startsWith('/') && it.directory != "/" &&
                it.directory.split('/').drop(1).none { part -> part.isEmpty() || part == "." || part == ".." || '\u0000' in part } &&
                it.file == "${it.directory}/tailscaled.state" })
            check(result.map { it.tag }.toSet().size == result.size && result.map { it.file }.toSet().size == result.size)
            return result.sortedWith(compareBy({ it.tag }, { it.directory }, { it.file }))
        }
        private fun observedRuns(array: JSONArray): List<Run> = (0 until array.length()).map { index ->
            val value = array.getJSONObject(index)
            Run(value.getString("runNonce"), value.getString("configDigest"), scopes(value.getJSONArray("scopes")))
        }.also { check(it.map { run -> run.nonce }.toSet().size == it.size && it.all { run -> hex64(run.nonce) && hex64(run.digest) }) }
        internal fun parseRuns(raw: String): List<Run> {
            check(raw.toByteArray(Charsets.UTF_8).size <= MAX_BYTES)
            val root = JSONObject(raw)
            check(root.getString("contractVersion") == "polaris-ts-auth-writer-retirement-v1" && root.getString("globalCleanupEvidence") == "CleanupUnknown")
            val instances = root.getJSONArray("instances")
            check(instances.length() <= 256)
            return (0 until instances.length()).map { index ->
                val instance = instances.getJSONObject(index)
                Run(instance.getString("runNonce"), instance.getString("configDigest"), scopes(instance.getJSONArray("nodes")))
            }.also { check(it.map { run -> run.nonce }.toSet().size == it.size && it.all { run -> hex64(run.nonce) && hex64(run.digest) }) }
        }
        /** Older AAR absence cannot create a receipt; ordinary operation still runs. */
        internal fun censusComplete(raw: String): Boolean {
            val instances = JSONObject(raw).getJSONArray("instances")
            return instances.length() > 0 && (0 until instances.length()).all { instances.getJSONObject(it).getBoolean("censusComplete") }
        }
        internal fun terminalComplete(raw: String): Boolean {
            val instances = JSONObject(raw).getJSONArray("instances")
            if (instances.length() == 0) return false
            for (index in 0 until instances.length()) {
                val instance = instances.getJSONObject(index)
                if (!instance.getBoolean("censusComplete") || instance.getString("terminal") !in KNOWN_WRITERS) return false
                val nodes = instance.getJSONArray("nodes")
                for (nodeIndex in 0 until nodes.length()) if (nodes.getJSONObject(nodeIndex).getString("writerState") !in KNOWN_WRITERS) return false
            }
            return true
        }
        fun noAuthConfig(config: String): Boolean = runCatching {
            val endpoints = JSONObject(config).getJSONArray("endpoints")
            (0 until endpoints.length()).all { index ->
                val endpoint = endpoints.getJSONObject(index)
                endpoint.optString("type") != "tailscale" || endpoint.optString("auth_key").isEmpty()
            }
        }.getOrDefault(false)
        fun export(server: Any): String = server.javaClass.getMethod("exportTailscaleStoreRetirement").invoke(server) as String
        internal fun canonicalScopes(array: JSONArray): List<Scope> = scopes(array)
    }
}

/** Both immutable getters belong only to this original native CheckConfig ticket. */
internal class AndroidTailscaleValidationScope private constructor(
    private val request: String, private val digest: String, private val membership: String, private val retirement: String,
    private val targets: List<AndroidTailscaleStoreCustody.Scope>, private val complete: Boolean, private val writersKnown: Boolean,
    private val validation: String, private val cleanup: String, private val noAuth: Boolean,
) {
    fun wire(): JSONObject = JSONObject().put("nativeContractVersion", "polaris-validation-v1").put("requestID", request).put("configDigest", digest)
        .put("validation", validation).put("cleanup", cleanup).put("membership", membership).put("storeRetirement", retirement)
    fun selectsWarm(file: String): Boolean = complete && writersKnown && validation == "Accepted" && noAuth && targets.any { it.file == file }
    fun related(file: String): Boolean? {
        if (validation == "Rejected" && cleanup == "NoConstruction") return false
        if (!complete) return null
        if (targets.none { it.file == file }) return false
        if (!writersKnown) return null
        return true
    }
    companion object {
        fun capture(request: String, config: String, result: Any): AndroidTailscaleValidationScope {
            fun get(name: String) = result.javaClass.getMethod(name).invoke(result) as String
            val digest = AndroidTailscaleStoreCustody.digest(config)
            check(get("getContractVersion") == "polaris-validation-v1" && get("getRequestID") == request && get("getConfigDigest") == digest)
            val validation = get("getValidation"); val cleanup = get("getCleanup")
            check(validation in setOf("Accepted", "Rejected", "InternalFailure") && cleanup in setOf("NoConstruction", "CleanupUnknown", "DisposedExact"))
            val member = get("getTailscaleStoreMembership")
            val retirement = get("getTailscaleStoreRetirement")
            check(member.toByteArray(Charsets.UTF_8).size <= AndroidTailscaleStoreCustody.MAX_BYTES)
            val document = JSONObject(member)
            check(document.keys().asSequence().toSet() == setOf("contractVersion", "requestID", "configDigest", "runNonce", "membershipState", "targets"))
            check(document.getString("contractVersion") == "polaris-ts-store-target-membership-v1" &&
                document.getString("requestID") == request && document.getString("configDigest") == digest)
            check(document.getString("membershipState") in setOf("Complete", "Unknown"))
            val nonce = document.getString("runNonce")
            val runs = AndroidTailscaleStoreCustody.parseRuns(retirement)
            val targets = AndroidTailscaleStoreCustody.canonicalScopes(document.getJSONArray("targets"))
            if (validation == "Rejected" && cleanup == "NoConstruction") {
                check(document.getString("membershipState") == "Unknown" && targets.isEmpty() &&
                    (nonce.isEmpty() || AndroidTailscaleStoreCustody.hex64(nonce))) { "nativeRetirementUnknown" }
            }
            val complete = get("getValidation") == "Accepted" && document.getString("membershipState") == "Complete" &&
                AndroidTailscaleStoreCustody.hex64(nonce) && runs.size == 1 && runs[0].nonce == nonce && runs[0].digest == digest &&
                runs[0].scopes == targets
            val writersKnown = runCatching { AndroidTailscaleStoreCustody.terminalComplete(retirement) }.getOrDefault(false)
            return AndroidTailscaleValidationScope(request, digest, member, retirement, targets, complete, writersKnown, validation, cleanup, AndroidTailscaleStoreCustody.noAuthConfig(config))
        }
    }
}
