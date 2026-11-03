package com.polaris2.app.vpn

import java.io.Closeable
import java.util.IdentityHashMap

/** V1 receipt belongs to Core's original physical graph, never to a caller supplied flag. */
internal interface NativeTunReceipt {
    val ownerId: String
    fun published(): Boolean
}

internal interface NativeTunSession {
    val ownerId: String
    fun prepare()
    fun fileDescriptor(): Int
    fun validate()
    fun abort()
    fun retire()
}

/** Serial native custody. Core joins its physical readers/duplicate before Abort or Retire. */
internal class NativeTunOwner<Settings, Descriptor : Closeable>(
    private val current: () -> Unit,
    private val stillOwner: () -> Unit,
    private val establish: (Settings) -> Descriptor,
    private val fd: (Descriptor) -> Int,
    private val scope: (Settings) -> TunScope,
    private val onUnknown: (Throwable) -> Unit,
) : Closeable {
    private val lock = Any()
    private val originals = IdentityHashMap<Descriptor, Unit>()
    private val sessions = mutableSetOf<Session>()
    private var applied: Session? = null
    private var pending: Session? = null
    private var legacySettings: Settings? = null
    private var legacyDescriptor: Descriptor? = null
    private var unknown: Throwable? = null
    private var closed = false

    private enum class Phase { New, Preparing, Prepared, Restored, Aborted, Retired }

    private fun healthy() {
        check(!closed) { "android: native TUN owner closed" }
        check(unknown == null) { "android: native TUN cleanup unknown" }
    }

    private fun quarantine(failure: Throwable): Nothing {
        if (unknown == null) {
            unknown = failure
            runCatching { onUnknown(failure) }
        }
        throw IllegalStateException("android: native TUN cleanup unknown", failure)
    }

    private fun own(descriptor: Descriptor): Descriptor {
        originals[descriptor] = Unit
        return descriptor
    }

    private fun release(descriptor: Descriptor?) {
        if (descriptor == null || !originals.containsKey(descriptor)) return
        descriptor.close()
        originals.remove(descriptor)
    }

    /** Used only when the selected AAR has no V1 registration method. */
    fun installLegacy(settings: Settings, descriptor: Descriptor): Int = synchronized(lock) {
        own(descriptor)
        try {
            healthy()
            current()
            check(sessions.isEmpty()) { "android: legacy openTun cannot replace a graph session" }
        } catch (failure: Throwable) {
            try { release(descriptor) } catch (cleanup: Throwable) { quarantine(cleanup) }
            throw failure
        }
        try {
            val previous = legacyDescriptor
            legacyDescriptor = descriptor
            legacySettings = settings
            release(previous)
            fd(descriptor)
        } catch (failure: Throwable) { quarantine(failure) }
    }

    fun newSession(settings: Settings, previous: NativeTunSession?, receipt: NativeTunReceipt): NativeTunSession = synchronized(lock) {
        healthy()
        current()
        check(legacyDescriptor == null) { "android: legacy TUN cannot enter graph migration" }
        check(receipt.ownerId.isNotBlank()) { "android: native TUN graph identity missing" }
        check(sessions.none { it.ownerId == receipt.ownerId }) { "android: native TUN graph identity reused" }
        // Core has suspended the exact previous physical, so its Published receipt may now be false.
        pending?.let { if (it === applied && previous === it && it.phase == Phase.Prepared) pending = null }
        check(pending == null) { "android: native TUN operation already pending" }
        check(previous === applied) { "android: native TUN previous graph differs" }
        applied?.let {
            check((it.phase == Phase.Prepared || it.phase == Phase.Restored) &&
                it.descriptor != null && originals.containsKey(it.descriptor)) { "android: previous native graph original unavailable" }
        }
        check(!receipt.published()) { "android: new native graph already published" }
        Session(settings, applied, receipt).also { sessions.add(it); pending = it }
    }

    fun currentScope(): TunScope? = synchronized(lock) {
        if (closed || unknown != null) return@synchronized null
        if (legacyDescriptor != null) return@synchronized legacySettings?.let(scope)
        val candidate = applied ?: return@synchronized null
        if (candidate.phase != Phase.Prepared && candidate.phase != Phase.Restored) return@synchronized null
        // Publication is a retained Core atomic receipt, not a native-side readiness bit.
        if (candidate.receipt.published()) scope(candidate.settings)
        else candidate.previous?.takeIf { it.receipt.published() }?.let { scope(it.settings) }
    }

    private inner class Session(
        val settings: Settings,
        var previous: Session?,
        val receipt: NativeTunReceipt,
    ) : NativeTunSession {
        override val ownerId = receipt.ownerId
        var phase = Phase.New
        var descriptor: Descriptor? = null

        override fun prepare() = synchronized(lock) {
            healthy()
            current()
            check(pending === this && applied === previous && phase == Phase.New) { "android: native TUN prepare owner differs" }
            phase = Phase.Preparing
            // establish may have changed OS settings even when it throws. Abort must replay old settings.
            applied = this
            descriptor = own(establish(settings))
            current() // Stop during establish cannot authorize a late descriptor or graph.
            phase = Phase.Prepared
        }

        override fun fileDescriptor(): Int = synchronized(lock) {
            healthy()
            current()
            check(applied === this && (phase == Phase.Prepared || phase == Phase.Restored)) { "android: native TUN descriptor is not applied" }
            fd(checkNotNull(descriptor))
        }

        override fun validate() = synchronized(lock) {
            healthy()
            current()
            check(applied === this && (pending === this || pending == null) &&
                (phase == Phase.Prepared || phase == Phase.Restored) &&
                descriptor != null && originals.containsKey(descriptor)) { "android: native TUN publication owner differs" }
        }

        override fun abort(): Unit = synchronized(lock) {
            if (phase == Phase.Aborted) return@synchronized
            healthy()
            stillOwner()
            check(!receipt.published()) { "android: published native graph cannot abort" }
            check(pending === this) { "android: native TUN abort operation differs" }
            if (phase == Phase.New) {
                pending = null
                phase = Phase.Aborted
                sessions.remove(this)
                return@synchronized
            }
            try {
                current() // A revoked attempt is closed by Stop, never restored by a late callback.
                check(applied === this) { "android: native TUN abort applied owner differs" }
                val old = previous
                if (old != null) {
                    val previousOriginal = old.descriptor
                    val restored = own(establish(old.settings))
                    old.descriptor = restored
                    current()
                    old.phase = Phase.Restored // Actual replay completed; the old physical still awaits Core rebuild/publication.
                    applied = old
                    release(previousOriginal)
                } else applied = null
                release(descriptor)
                descriptor = null
                pending = null
                phase = Phase.Aborted
                sessions.remove(this)
            } catch (failure: Throwable) { quarantine(failure) }
        }

        override fun retire(): Unit = synchronized(lock) {
            if (phase == Phase.Retired || phase == Phase.Aborted) return@synchronized
            healthy()
            stillOwner() // Exact Stop may revoke admission before native Close calls Retire.
            try {
                check(phase == Phase.Prepared || phase == Phase.Restored) { "android: unprepared native graph cannot retire" }
                // Core's retained everPublished history chooses Retire; Published itself is false while suspended.
                if (applied === this) applied = null
                else check(applied?.previous === this) { "android: native TUN successor differs" }
                // Superseded originals never clear the newer Builder/settings owner.
                release(descriptor)
                descriptor = null
                if (applied?.previous === this) applied?.previous = null // Retired snapshots/receipts do not form an unbounded chain.
                previous = null
                if (pending === this) pending = null
                phase = Phase.Retired
                sessions.remove(this)
            } catch (failure: Throwable) { quarantine(failure) }
        }
    }

    /** Only BoxService's successful native close/join may invoke this terminal cleanup. */
    override fun close() = synchronized(lock) {
        if (closed) return@synchronized
        healthy()
        stillOwner()
        try {
            originals.keys.toList().forEach(::release)
            applied = null
            pending = null
            legacyDescriptor = null
            legacySettings = null
            sessions.clear()
            closed = true
        } catch (failure: Throwable) { quarantine(failure) }
    }
}
