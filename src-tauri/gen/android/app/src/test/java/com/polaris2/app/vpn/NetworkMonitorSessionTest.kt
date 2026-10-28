package com.polaris2.app.vpn

import org.junit.Assert.*
import org.junit.Test
import org.junit.After
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

class NetworkMonitorSessionTest {
    private data class Proxy(val id: String, val proxy: Int = 0)
    private val ownedSessions = mutableListOf<NetworkMonitorSession<*, *>>()
    private val owners = java.util.IdentityHashMap<NetworkMonitorSession<*, *>, MutableList<Pair<Thread, AtomicReference<Throwable?>>>>()
    private val holds = mutableListOf<CountDownLatch>()
    private val callers = mutableListOf<Pair<Thread, AtomicReference<Throwable?>>>()

    private fun releaseLatch() = CountDownLatch(1).also { holds += it }

    private fun <N, L> own(session: NetworkMonitorSession<N, L>): NetworkMonitorSession<N, L> {
        val workers = Collections.synchronizedList(mutableListOf<Pair<Thread, AtomicReference<Throwable?>>>())
        ownedSessions += session
        owners[session] = workers
        // Observe actual executor owners, never insert a sentinel into its capacity-1
        // queue. Zero-core workers retire at idle in this fixture, without a sleep
        // or queue-capacity change; production scheduling remains asynchronous.
        session.worker.setKeepAliveTime(0, TimeUnit.MILLISECONDS)
        session.worker.threadFactory = java.util.concurrent.ThreadFactory { action ->
            val failure = AtomicReference<Throwable?>()
            val owner = Thread {
                try { action.run() } catch (error: Throwable) { failure.set(error) }
            }
            workers += owner to failure // recorded before ThreadPoolExecutor starts it
            owner
        }
        return session
    }

    private fun awaitIdle(session: NetworkMonitorSession<*, *>) {
        val workers = checkNotNull(owners[session])
        var joined = 0
        while (true) {
            val batch = synchronized(workers) { workers.drop(joined) }
            if (batch.isEmpty()) break
            batch.forEach(::finish)
            joined += batch.size
            // A dying executor owner can create its successor before returning.
            // Join that exact owner too, rather than polling flags or submitting work.
        }
        assertEquals(0, session.snapshot().lookups)
        assertEquals(0, session.snapshot().delivering)
        assertTrue(session.worker.queue.isEmpty())
    }

    @After fun releaseAndRetireEveryFixtureOwner() {
        holds.forEach { it.countDown() }
        var failure: Throwable? = null
        fun cleanup(action: () -> Unit) {
            try { action() } catch (error: Throwable) { if (failure == null) failure = error }
        }
        callers.forEach { cleanup { finish(it) } }
        ownedSessions.forEach { cleanup { it.stop() } }
        ownedSessions.forEach { session -> cleanup {
            awaitIdle(session)
            assertTrue(session.worker.awaitTermination(5, TimeUnit.SECONDS))
        } }
        failure?.let { throw AssertionError("fixture owner cleanup failed", it) }
    }

    private inner class Harness {
        val events = mutableListOf<NetworkMonitorEvents<String>>()
        var initial: () -> String? = { null }
        var lookup: (String) -> NetworkMonitorUpdate? = { NetworkMonitorUpdate(it, it.length) }
        var delivery: (Proxy, NetworkMonitorUpdate) -> Unit = { _, _ -> }
        var registration: () -> Unit = {}
        var unregistration: () -> Unit = {}
        var registered = 0
        var unregistered = 0
        val calls = Collections.synchronizedList(mutableListOf<Pair<String, String>>())
        val owner = NetworkMonitorCoordinator<String, Proxy>(
            registration = { callback ->
                events += callback
                object : NetworkMonitorRegistration {
                    override fun register() { registered++; registration() }
                    override fun unregister() { unregistered++; unregistration() }
                }
            },
            initialNetwork = { initial() }, lookup = { lookup(it) }, identity = { it.id },
            deliver = { proxy, update -> calls += proxy.id to update.name; delivery(proxy, update) },
        )
        val sessions = mutableListOf<NetworkMonitorSession<String, Proxy>>()
        fun newSession() = own(owner.createSession()).also { sessions += it }
        fun drain() = sessions.forEach { awaitIdle(it) }
        fun session(id: String = "1"): NetworkMonitorSession<String, Proxy> = newSession().also {
            it.start(); it.startListener(Proxy(id)); awaitIdle(it); calls.clear()
        }
    }
    private fun await(latch: CountDownLatch) { assertTrue("deterministic latch timed out", latch.await(5, TimeUnit.SECONDS)) }
    private fun thread(action: () -> Unit): Pair<Thread, AtomicReference<Throwable?>> {
        val failure = AtomicReference<Throwable?>()
        val worker = Thread { try { action() } catch (error: Throwable) { failure.set(error) } }
        val owned = worker to failure
        callers += owned
        worker.start()
        return owned
    }
    private fun finish(worker: Pair<Thread, AtomicReference<Throwable?>>) {
        worker.first.join(5000)
        assertFalse("worker still in flight", worker.first.isAlive)
        worker.second.get()?.let { throw AssertionError(it) }
    }

    @Test fun capturedLookupAndQueuedOldSdkCallbackCannotReachSuccessor() {
        val h = Harness(); val a = h.session()
        val entered = CountDownLatch(1); val release = releaseLatch()
        h.lookup = { if (it == "old") { entered.countDown(); await(release) }; NetworkMonitorUpdate(it, 1) }
        val worker = thread { h.events[0].available("old") }; await(entered)
        a.beginClose(); a.stop()
        val b = h.session("2"); h.events[1].available("same")
        release.countDown(); finish(worker); h.drain()
        h.events[0].available("same"); h.events[0].lost("same")
        assertEquals("same", b.defaultNetwork)
        assertEquals("same", h.owner.defaultNetwork)
        assertEquals(listOf("2" to "same"), h.calls.toList())
        a.stop(); assertEquals("same", b.defaultNetwork)
        assertEquals(1, h.unregistered)
    }

    @Test fun revokeBeforePermitMakesZeroJniAndNativeIdentityReplacementIsExact() {
        val h = Harness(); val a = h.session()
        val entered = CountDownLatch(1); val release = releaseLatch()
        h.lookup = { entered.countDown(); await(release); NetworkMonitorUpdate(it, 1) }
        val worker = thread { h.events[0].available("old") }; await(entered)
        a.closeListener(Proxy("1", 99)) // a different Java wrapper of the same native identity
        release.countDown(); finish(worker); h.drain()
        assertTrue(h.calls.isEmpty())
        h.lookup = { NetworkMonitorUpdate(it, 2) }
        a.startListener(Proxy("2")); h.drain(); h.calls.clear()
        a.closeListener(Proxy("1", 100)); h.events[0].available("new"); h.drain()
        assertEquals(listOf("2" to "new"), h.calls.toList())
        try { a.startListener(Proxy("1")); fail("old incarnation revived") } catch (_: IllegalStateException) {}
    }

    @Test fun deliveringPermitRemainsUntilActualJniReturnAndFenceNeverWaits() {
        val h = Harness(); val a = h.session()
        val entered = CountDownLatch(1); val release = releaseLatch()
        h.delivery = { _, update -> if (update.name == "old") { entered.countDown(); await(release) } }
        val worker = thread { h.events[0].available("old") }; await(entered)
        a.beginClose()
        assertEquals(1, a.snapshot().delivering)
        assertEquals(listOf("1" to "old"), h.calls.toList())
        val b = h.session("2"); h.events[1].available("new")
        assertEquals("new", b.defaultNetwork)
        release.countDown(); finish(worker); h.drain()
        assertEquals(0, a.snapshot().delivering)
        assertEquals(0, a.snapshot().lookups)
        assertEquals(listOf("2" to "new"), h.calls.toList())
    }

    @Test fun latestEventIsDeliveredAfterBusyJniWithoutOutOfOrderUpdates() {
        val h = Harness(); h.session()
        val entered = CountDownLatch(1); val release = releaseLatch()
        h.delivery = { _, update -> if (update.name == "first") { entered.countDown(); await(release) } }
        val worker = thread { h.events[0].available("first") }; await(entered)
        h.events[0].available("middle"); h.events[0].available("last")
        assertEquals(listOf("1" to "first"), h.calls.toList())
        release.countDown(); finish(worker); h.drain()
        assertEquals(listOf("1" to "first", "1" to "last"), h.calls.toList())
    }

    @Test fun staleSlowLookupAndInitialSnapshotCannotOverrideNewEvent() {
        val h = Harness()
        val entered = CountDownLatch(1); val release = releaseLatch()
        h.initial = { entered.countDown(); await(release); "stale" }
        val a = h.newSession()
        val worker = thread { a.start() }; await(entered)
        h.events[0].available("current"); release.countDown(); finish(worker); h.drain()
        a.startListener(Proxy("1")); awaitIdle(a); assertEquals("current", a.defaultNetwork)
        assertEquals(listOf("1" to "current"), h.calls.toList())
        val slow = CountDownLatch(1); val resume = releaseLatch()
        h.lookup = { if (it == "slow") { slow.countDown(); await(resume) }; NetworkMonitorUpdate(it, 1) }
        val lookupWorker = thread { h.events[0].available("slow") }; await(slow)
        h.events[0].available("latest"); resume.countDown(); finish(lookupWorker); h.drain()
        assertEquals("1" to "latest", h.calls.last())
        assertFalse(h.calls.any { it.second == "slow" })
    }

    @Test fun lateLostAndCapabilitiesUseExactCurrentNetwork() {
        val h = Harness(); h.session()
        h.events[0].available("wifi"); h.events[0].available("cellular")
        h.events[0].lost("wifi"); h.events[0].capabilitiesChanged("wifi")
        assertEquals("cellular", h.owner.defaultNetwork)
        h.events[0].capabilitiesChanged("cellular"); h.events[0].lost("cellular"); h.drain()
        assertNull(h.owner.defaultNetwork)
        assertEquals("1" to "", h.calls.last())
    }

    @Test fun closeBeforeStartAndLateStartCannotReopenNativeIncarnation() {
        val h = Harness(); val a = h.newSession()
        a.closeListener(Proxy("4"))
        try { a.startListener(Proxy("4", 10)); fail() } catch (_: IllegalStateException) {}
        a.start(); a.startListener(Proxy("5")); a.startListener(Proxy("5", 20)); awaitIdle(a)
        assertEquals(1, h.calls.size)
        a.beginClose()
        try { a.startListener(Proxy("6")); fail() } catch (_: IllegalStateException) {}
        try { a.start(); fail() } catch (_: IllegalStateException) {}
    }

    @Test fun unknownNativeIdentityIsRejectedWithoutChangingCurrentListener() {
        val h = Harness(); val a = h.session()
        for (invalid in listOf("", "foreign", "0", "-1", "01", "9223372036854775808")) {
            try { a.closeListener(Proxy(invalid)); fail(invalid) } catch (_: IllegalArgumentException) {}
        }
        h.events[0].available("wifi"); h.drain()
        assertEquals(listOf("1" to "wifi"), h.calls.toList())
    }

    @Test fun stopDuringRegistrationCompensatesExactCallbackAfterRealReturn() {
        val h = Harness(); val a = h.newSession()
        val entered = CountDownLatch(1); val release = releaseLatch()
        h.registration = { entered.countDown(); await(release) }
        val worker = thread { a.start() }; await(entered)
        a.stop(); assertTrue(a.snapshot().registering); assertEquals(0, h.unregistered)
        val b = h.newSession()
        h.registration = {}; b.start(); b.startListener(Proxy("2")); h.events[1].available("new")
        release.countDown(); finish(worker); h.drain()
        assertEquals(1, h.unregistered)
        assertEquals("new", h.owner.defaultNetwork)
        assertFalse(a.snapshot().registering)
    }

    @Test fun registrationFailureKeepsFirstErrorAndLateCallbackCannotPublish() {
        val h = Harness(); val a = h.newSession()
        val first = IllegalStateException("register")
        h.registration = { throw first }; h.unregistration = { throw IllegalArgumentException("unregister") }
        try { a.start(); fail() } catch (error: Throwable) { assertSame(first, error) }
        assertSame(first, a.snapshot().firstFailure)
        h.events[0].available("late")
        assertNull(h.owner.defaultNetwork)
        assertTrue(a.snapshot().revoked)
    }

    @Test fun callbackCanCloseItselfAndFailuresKeepLatestPendingUpdate() {
        val h = Harness(); val a = h.session()
        h.delivery = { _, _ -> a.closeListener(Proxy("1", 30)); a.stop() }
        h.events[0].available("self"); h.drain()
        assertEquals(0, a.snapshot().delivering); assertEquals(0, a.snapshot().lookups)
        val b = h.session("2")
        val first = IllegalStateException("jni")
        h.delivery = { _, update -> if (update.name == "first") {
            h.events[1].available("latest"); throw first
        } }
        h.events[1].available("first"); h.drain() // asynchronous failure is retained in the exact session
        assertEquals("2" to "latest", h.calls.last())
        assertSame(first, b.snapshot().firstFailure)
        assertEquals(0, b.snapshot().delivering)
    }

    @Test fun s5ObservationCountsBeforeBusyJniAndFencesLateCapabilitiesAcrossStopStart() {
        val h = Harness(); val a = h.session()
        h.events[0].available("a"); h.events[0].capabilitiesChanged("a", true); h.drain()
        val source = h.owner.networkObservation().sourceEpoch
        val entered = CountDownLatch(1); val release = releaseLatch()
        h.delivery = { _, update -> if (update.name == "b") { entered.countDown(); await(release) } }
        val worker = thread { h.events[0].available("b") }; await(entered)
        try {
            h.events[0].capabilitiesChanged("b", true)
            h.events[0].available("a"); h.events[0].capabilitiesChanged("a", true)
            assertEquals("2", h.owner.networkObservation().seq)
            a.beginClose(); a.stop()
            assertFalse(h.owner.networkObservation().currentKnown)
            h.delivery = { _, _ -> } // Only the captured old JNI call waits on this latch.
            val b = h.session("2")
            h.events[1].available("b"); h.events[1].capabilitiesChanged("b", true)
            h.events[0].available("old"); h.events[0].capabilitiesChanged("old", true)
            a.stop()
            assertEquals("b", b.defaultNetwork)
            assertEquals("3", h.owner.networkObservation().seq)
            assertEquals(source, h.owner.networkObservation().sourceEpoch)
        } finally { release.countDown(); finish(worker); h.drain() }
    }

    @Test fun s5UnfilteredBootstrapAndFailedRegistrationNeverSupplyKnownNetworkProof() {
        val h = Harness(); h.initial = { "unfiltered-vpn" }; val a = h.session()
        h.events[0].capabilitiesChanged("unfiltered-vpn", true)
        assertFalse(h.owner.networkObservation().currentKnown)
        a.stop()
        h.registration = {
            h.events.last().available("a"); h.events.last().capabilitiesChanged("a", true)
            throw IllegalStateException("partial register")
        }
        try { h.newSession().start(); fail("registration should fail") } catch (_: IllegalStateException) {}
        assertFalse(h.owner.networkObservation().currentKnown)
        val source = h.owner.networkObservation().sourceEpoch
        h.registration = {}; val next = h.session("2")
        h.events.last().available("a"); h.events.last().capabilitiesChanged("a", true)
        h.events[1].available("late"); h.events[1].capabilitiesChanged("late", true)
        next.start() // duplicate registration does not reset producer
        assertEquals("0", h.owner.networkObservation().seq)
        assertEquals(source, h.owner.networkObservation().sourceEpoch)
        assertTrue(h.owner.networkObservation().currentKnown)
        assertEquals(3, h.registered)
    }

    @Test fun actualAttemptRevocationFencesMonitorWhileOperationLockAndJniAreBusy() {
        val entered = CountDownLatch(1); val release = releaseLatch()
        val coordinator = NetworkMonitorCoordinator<android.net.Network, io.nekohasekai.libbox.InterfaceUpdateListener>(
            registration = { object : NetworkMonitorRegistration {
                override fun register() {}
                override fun unregister() {}
            } },
            initialNetwork = { null }, lookup = { error("unexpected SDK lookup") },
            identity = { "100" }, deliver = { _, _ -> entered.countDown(); await(release) },
        )
        val session = own(coordinator.createSession()); session.start()
        val attempt = MainKernelAttempt<Any>(network = session)
        val listener = object : io.nekohasekai.libbox.InterfaceUpdateListener {
            override fun updateDefaultInterface(name: String, index: Int, expensive: Boolean, constrained: Boolean) {}
            override fun updateNetworkPath(path: String) {}
        }
        val lockEntered = CountDownLatch(1)
        val lockRelease = releaseLatch()
        val lockOwner = thread {
            synchronized(attempt.operationLock) {
                session.startListener(listener)
                lockEntered.countDown()
                await(lockRelease)
            }
        }
        try {
            await(lockEntered)
            await(entered) // independently proves native delivery owns its permit
            assertEquals(1L, lockRelease.count)
            assertEquals(1L, release.count)
            // The lock owner cannot leave its synchronized block, and native
            // delivery cannot return, until this test releases their separate holds.
            attempt.revokeAndDetachTun()
            assertTrue(attempt.revoked)
            assertTrue(session.snapshot().revoked)
            assertEquals(1, session.snapshot().delivering)
        } finally {
            release.countDown()
            lockRelease.countDown()
            finish(lockOwner)
            awaitIdle(session)
            session.stop()
            assertTrue(session.worker.awaitTermination(5, TimeUnit.SECONDS))
        }
        assertEquals(0, session.snapshot().delivering)
        assertEquals(0, session.snapshot().lookups)
    }
    @Test fun callbackReturnsWhileLookupBlockedAndReplacementCoalescesOnOnePump() {
        val h = Harness(); val session = h.session()
        val entered = CountDownLatch(1); val release = releaseLatch()
        h.lookup = { network ->
            if (network == "blocked") { entered.countDown(); await(release) }
            NetworkMonitorUpdate(network, 1)
        }
        try {
            // Same calling thread can immediately execute a sentinel and stop fence.
            h.events[0].available("blocked")
            await(entered)
            assertEquals(1, session.snapshot().lookups)
            for (id in 2..100) {
                session.startListener(Proxy(id.toString()))
                h.events[0].available("network-$id")
            }
            assertEquals(1, session.snapshot().lookups)
            assertTrue(session.worker.queue.isEmpty())
            assertEquals("network-100", session.defaultNetwork)
        } finally { release.countDown(); h.drain() }
        assertEquals(listOf("100" to "network-100"), h.calls.toList())
        session.stop()
    }

    @Test fun lookupStallDoesNotDelayCallbackThreadStopAndCannotPublishAfterFence() {
        val h = Harness(); val session = h.session()
        val entered = CountDownLatch(1); val release = releaseLatch()
        h.lookup = { entered.countDown(); await(release); NetworkMonitorUpdate(it, 1) }
        try {
            h.events[0].available("blocked")
            await(entered)
            session.beginClose() // returns on the original callback/main thread
            assertTrue(session.snapshot().revoked)
            assertEquals(1, session.snapshot().lookups)
            assertTrue(h.calls.isEmpty())
        } finally { release.countDown(); h.drain(); session.stop() }
        assertEquals(0, session.snapshot().lookups)
        assertTrue(h.calls.isEmpty())
    }

}
