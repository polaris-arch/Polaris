package com.polaris2.app.vpn

/** A fresh platform callback belongs to one SDK registration of one attempt. */
internal interface NetworkMonitorEvents<N> {
    fun available(network: N)
    fun capabilitiesChanged(network: N, notVpn: Boolean = false)
    fun lost(network: N)
}

internal interface NetworkMonitorRegistration {
    fun register()
    fun unregister()
}

internal data class NetworkMonitorUpdate(val name: String, val index: Int,
    val expensive: Boolean = false, val constrained: Boolean = false)

internal data class NetworkMonitorSnapshot(val revoked: Boolean, val lookups: Int,
    val delivering: Int, val registering: Boolean, val registered: Boolean,
    val firstFailure: Throwable?)

/** Exact process-lived history, guarded by the coordinator metadata gate. Network identities stay private. */
internal data class DefaultNetworkObservationSnapshot(val sourceEpoch: String, val seq: String,
    val currentKnown: Boolean, val coverageGap: Boolean)

internal class DefaultNetworkObservation<N>(private val sourceEpoch: String = java.util.UUID.randomUUID().toString()) {
    private var session: Any? = null
    private var candidate: N? = null
    private var anchor: N? = null
    private var seq = 0L
    private var currentKnown = false
    private var coverageGap = true

    fun start(token: Any) { session = token; candidate = null; currentKnown = false }
    fun available(token: Any, network: N) {
        if (session !== token) return
        candidate = network
        currentKnown = false
    }
    fun capabilities(token: Any, network: N, notVpn: Boolean) {
        if (session !== token || candidate != network) return
        if (!notVpn) { currentKnown = false; coverageGap = true; return }
        val previous = anchor
        if (previous != null && previous != network) {
            // Exhaustion becomes unavailable rather than wrapping and inventing a baseline.
            if (seq == Long.MAX_VALUE) { currentKnown = false; coverageGap = true; return }
            seq++
        }
        anchor = network
        currentKnown = true
    }
    fun lost(token: Any, network: N) {
        if (session !== token || candidate != network) return
        candidate = null
        currentKnown = false
        coverageGap = true
    }
    fun stop(token: Any) {
        if (session !== token) return
        session = null
        candidate = null
        currentKnown = false
        coverageGap = true
        // Keep source, known anchor and every confirmed change across attempt restarts.
    }
    fun snapshot() = DefaultNetworkObservationSnapshot(sourceEpoch, seq.toString(), currentKnown, coverageGap)
}

/** All publication/revocation/permits use this small metadata gate. SDK and JNI never hold it. */
internal class NetworkMonitorCoordinator<N, L>(
    private val registration: (NetworkMonitorEvents<N>) -> NetworkMonitorRegistration,
    private val initialNetwork: () -> N?,
    private val lookup: (N) -> NetworkMonitorUpdate?,
    private val identity: (L) -> String,
    private val deliver: (L, NetworkMonitorUpdate) -> Unit,
) {
    private val gate = Any()
    private var active: NetworkMonitorSession<N, L>? = null
    private val observation = DefaultNetworkObservation<N>()
    fun networkObservation() = synchronized(gate) { observation.snapshot() }
    val defaultNetwork: N? get() = synchronized(gate) { active?.network }

    fun createSession(): NetworkMonitorSession<N, L> = NetworkMonitorSession(this)

    internal fun makeRegistration(session: NetworkMonitorSession<N, L>): NetworkMonitorRegistration =
        registration(object : NetworkMonitorEvents<N> {
            override fun available(network: N) = event(session, network, 0)
            override fun capabilitiesChanged(network: N, notVpn: Boolean) = event(session, network, 1, notVpn)
            override fun lost(network: N) = event(session, network, 2)
        })

    internal fun start(session: NetworkMonitorSession<N, L>) {
        val revision = synchronized(gate) {
            check(!session.revoked) { "android: 默认网络 session 已撤销" }
            if (session.started) return
            check(active == null || active === session) { "android: 默认网络仍归上一 attempt" }
            session.started = true
            session.registering = true
            active = session
            observation.start(session)
            session.revision
        }
        try {
            session.sdk.register()
        } catch (error: Throwable) {
            synchronized(gate) {
                session.registering = false
                recordFailure(session, error)
            }
            fence(session)
            // Registration may throw after publishing a request. Revoke before
            // exact compensation, and keep the first error if compensation fails.
            unregister(session)
            throw error
        }
        val revoked = synchronized(gate) {
            session.registering = false
            session.registered = true
            session.revoked
        }
        if (revoked) {
            stop(session)
            return
        }
        val initial = try { initialNetwork() } catch (error: Throwable) {
            synchronized(gate) { recordFailure(session, error) }
            stop(session)
            throw error
        }
        val launch = synchronized(gate) {
            // A callback received during register/activeNetwork lookup wins.
            if (active !== session || session.revoked || session.revision != revision) null
            else {
                session.network = initial
                session.revision++
                enqueue(session)
            }
        }
        launch?.let { pump(session, it) }
    }

    private fun event(session: NetworkMonitorSession<N, L>, network: N, kind: Int, notVpn: Boolean = false) {
        val launch = synchronized(gate) {
            if (active !== session || session.revoked || !session.started) return
            // The callback object is immutable and unique to this session's SDK
            // registration. Even equal Network values cannot cross this identity.
            if (kind != 0 && session.network != network) return
            when (kind) {
                0 -> observation.available(session, network)
                1 -> observation.capabilities(session, network, notVpn)
                2 -> observation.lost(session, network)
            }
            session.network = if (kind == 2) null else network
            session.revision++
            enqueue(session)
        }
        launch?.let { pump(session, it) }
    }

    internal fun startListener(session: NetworkMonitorSession<N, L>, listener: L) {
        val incarnation = trustedIdentity(listener)
        val launch = synchronized(gate) {
            check(!session.revoked) { "android: 默认网络 session 已撤销" }
            val previous = session.current
            if (previous?.identity == incarnation) return // proxy identity is irrelevant
            check(incarnation > session.closedThrough && incarnation > session.startedThrough) {
                "android: 默认网络 native incarnation 已撤销或过时"
            }
            previous?.let { seal(it) }
            session.startedThrough = incarnation
            session.current = NetworkMonitorSession.Listener(incarnation, listener)
            if (active === session && session.started) enqueue(session) else null
        }
        launch?.let { pump(session, it) }
    }

    internal fun closeListener(session: NetworkMonitorSession<N, L>, listener: L) {
        val incarnation = trustedIdentity(listener)
        synchronized(gate) {
            // This entry is only bound to the original Go platform/session, never
            // an arbitrary bridge ID. Close before a delayed Start is remembered.
            session.closedThrough = maxOf(session.closedThrough, incarnation)
            session.current?.takeIf { it.identity == incarnation }?.let {
                seal(it)
                session.current = null
            }
        }
    }

    private fun trustedIdentity(listener: L): Long {
        val value = identity(listener)
        val number = value.toLongOrNull()
        require(number != null && number > 0 && number.toString() == value) {
            "android: 无效默认网络 native incarnation"
        }
        return number
    }

    private fun enqueue(session: NetworkMonitorSession<N, L>): NetworkMonitorSession.Listener<N, L>? {
        val listener = session.current ?: return null
        if (listener.sealed) return null
        listener.pending = NetworkMonitorSession.Work(session.network, session.revision)
        if (listener.running) return null
        listener.running = true
        session.lookups++
        return listener
    }

    private fun seal(listener: NetworkMonitorSession.Listener<N, L>) {
        listener.sealed = true
        listener.pending = null
        listener.value = null // an already permitted JNI stack keeps its exact local value
    }

    private fun pump(session: NetworkMonitorSession<N, L>, listener: NetworkMonitorSession.Listener<N, L>) {
        var failure: Throwable? = null
        while (true) {
            val work = synchronized(gate) {
                val pending = listener.pending
                listener.pending = null
                if (pending == null || listener.sealed || session.revoked) {
                    listener.running = false
                    session.lookups--
                    null
                } else pending
            }
            if (work == null) {
                failure?.let { throw it }
                return
            }
            try {
                val update = if (work.network == null) NetworkMonitorUpdate("", -1) else lookup(work.network)
                if (update == null) continue
                val target = synchronized(gate) {
                    if (active !== session || session.revoked || listener.sealed ||
                        session.current !== listener || session.revision != work.revision) null
                    else listener.value?.also { session.delivering++ }
                } ?: continue
                try { deliver(target, update) }
                finally { synchronized(gate) { session.delivering-- } }
            } catch (error: Throwable) {
                synchronized(gate) { recordFailure(session, error) }
                if (failure == null) failure = error
                // Drain the latest retained event after actual JNI/lookup return;
                // error handling cannot strand an event queued during delivery.
            }
        }
    }

    internal fun fence(session: NetworkMonitorSession<N, L>) = synchronized(gate) {
        session.revoked = true
        session.current?.let { seal(it) }
        session.current = null
        session.network = null
        if (active === session) { observation.stop(session); active = null }
    }

    internal fun stop(session: NetworkMonitorSession<N, L>) {
        fence(session)
        val unregister = synchronized(gate) {
            if (!session.registered) false else { session.registered = false; true }
        }
        if (unregister) unregister(session)
    }

    private fun unregister(session: NetworkMonitorSession<N, L>) {
        try { session.sdk.unregister() }
        catch (error: Throwable) { synchronized(gate) { recordFailure(session, error) } }
    }

    private fun recordFailure(session: NetworkMonitorSession<N, L>, error: Throwable) {
        if (session.firstFailure == null) session.firstFailure = error
    }

    internal fun network(session: NetworkMonitorSession<N, L>): N? = synchronized(gate) { session.network }
    internal fun snapshot(session: NetworkMonitorSession<N, L>) = synchronized(gate) {
        NetworkMonitorSnapshot(session.revoked, session.lookups, session.delivering,
            session.registering, session.registered, session.firstFailure)
    }
}

internal class NetworkMonitorSession<N, L> internal constructor(private val owner: NetworkMonitorCoordinator<N, L>) {
    internal class Listener<N, L>(val identity: Long, var value: L?) {
        var sealed = false
        var running = false
        var pending: Work<N>? = null
    }
    internal data class Work<N>(val network: N?, val revision: Long)
    internal var network: N? = null
    internal var revision = 0L
    internal var revoked = false
    internal var started = false
    internal var registering = false
    internal var registered = false
    internal var current: Listener<N, L>? = null
    internal var startedThrough = 0L
    internal var closedThrough = 0L
    internal var lookups = 0
    internal var delivering = 0
    internal var firstFailure: Throwable? = null
    internal val sdk = owner.makeRegistration(this)
    val defaultNetwork: N? get() = owner.network(this)
    fun start() = owner.start(this)
    fun startListener(listener: L) = owner.startListener(this, listener)
    fun closeListener(listener: L) = owner.closeListener(this, listener)
    fun beginClose() = owner.fence(this)
    fun stop() = owner.stop(this)
    fun snapshot() = owner.snapshot(this)
}
