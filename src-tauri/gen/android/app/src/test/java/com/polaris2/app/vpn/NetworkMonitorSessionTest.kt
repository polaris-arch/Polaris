package com.polaris2.app.vpn

import org.junit.Assert.*
import org.junit.Test
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

class NetworkMonitorSessionTest {
    private data class Proxy(val id: String, val proxy: Int = 0)
    private class Harness {
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
        fun session(id: String = "1"): NetworkMonitorSession<String, Proxy> = owner.createSession().also {
            it.start(); it.startListener(Proxy(id)); calls.clear()
        }
    }
    private fun await(latch: CountDownLatch) { assertTrue("deterministic latch timed out", latch.await(5, TimeUnit.SECONDS)) }
    private fun thread(action: () -> Unit): Pair<Thread, AtomicReference<Throwable?>> {
        val failure = AtomicReference<Throwable?>()
        val worker = Thread { try { action() } catch (error: Throwable) { failure.set(error) } }
        worker.start()
        return worker to failure
    }
    private fun finish(worker: Pair<Thread, AtomicReference<Throwable?>>) {
        worker.first.join(5000)
        assertFalse("worker still in flight", worker.first.isAlive)
        worker.second.get()?.let { throw AssertionError(it) }
    }

    @Test fun capturedLookupAndQueuedOldSdkCallbackCannotReachSuccessor() {
        val h = Harness(); val a = h.session()
        val entered = CountDownLatch(1); val release = CountDownLatch(1)
        h.lookup = { if (it == "old") { entered.countDown(); await(release) }; NetworkMonitorUpdate(it, 1) }
        val worker = thread { h.events[0].available("old") }; await(entered)
        a.beginClose(); a.stop()
        val b = h.session("2"); h.events[1].available("same")
        release.countDown(); finish(worker)
        h.events[0].available("same"); h.events[0].lost("same")
        assertEquals("same", b.defaultNetwork)
        assertEquals("same", h.owner.defaultNetwork)
        assertEquals(listOf("2" to "same"), h.calls.toList())
        a.stop(); assertEquals("same", b.defaultNetwork)
        assertEquals(1, h.unregistered)
    }

    @Test fun revokeBeforePermitMakesZeroJniAndNativeIdentityReplacementIsExact() {
        val h = Harness(); val a = h.session()
        val entered = CountDownLatch(1); val release = CountDownLatch(1)
        h.lookup = { entered.countDown(); await(release); NetworkMonitorUpdate(it, 1) }
        val worker = thread { h.events[0].available("old") }; await(entered)
        a.closeListener(Proxy("1", 99)) // a different Java wrapper of the same native identity
        release.countDown(); finish(worker)
        assertTrue(h.calls.isEmpty())
        h.lookup = { NetworkMonitorUpdate(it, 2) }
        a.startListener(Proxy("2")); h.calls.clear()
        a.closeListener(Proxy("1", 100)); h.events[0].available("new")
        assertEquals(listOf("2" to "new"), h.calls.toList())
        try { a.startListener(Proxy("1")); fail("old incarnation revived") } catch (_: IllegalStateException) {}
    }

    @Test fun deliveringPermitRemainsUntilActualJniReturnAndFenceNeverWaits() {
        val h = Harness(); val a = h.session()
        val entered = CountDownLatch(1); val release = CountDownLatch(1)
        h.delivery = { _, update -> if (update.name == "old") { entered.countDown(); await(release) } }
        val worker = thread { h.events[0].available("old") }; await(entered)
        a.beginClose()
        assertEquals(1, a.snapshot().delivering)
        assertEquals(listOf("1" to "old"), h.calls.toList())
        val b = h.session("2"); h.events[1].available("new")
        assertEquals("new", b.defaultNetwork)
        release.countDown(); finish(worker)
        assertEquals(0, a.snapshot().delivering)
        assertEquals(0, a.snapshot().lookups)
        assertEquals(listOf("2" to "new"), h.calls.toList())
    }

    @Test fun latestEventIsDeliveredAfterBusyJniWithoutOutOfOrderUpdates() {
        val h = Harness(); h.session()
        val entered = CountDownLatch(1); val release = CountDownLatch(1)
        h.delivery = { _, update -> if (update.name == "first") { entered.countDown(); await(release) } }
        val worker = thread { h.events[0].available("first") }; await(entered)
        h.events[0].available("middle"); h.events[0].available("last")
        assertEquals(listOf("1" to "first"), h.calls.toList())
        release.countDown(); finish(worker)
        assertEquals(listOf("1" to "first", "1" to "last"), h.calls.toList())
    }

    @Test fun staleSlowLookupAndInitialSnapshotCannotOverrideNewEvent() {
        val h = Harness()
        val entered = CountDownLatch(1); val release = CountDownLatch(1)
        h.initial = { entered.countDown(); await(release); "stale" }
        val a = h.owner.createSession()
        val worker = thread { a.start() }; await(entered)
        h.events[0].available("current"); release.countDown(); finish(worker)
        a.startListener(Proxy("1")); assertEquals("current", a.defaultNetwork)
        assertEquals(listOf("1" to "current"), h.calls.toList())
        val slow = CountDownLatch(1); val resume = CountDownLatch(1)
        h.lookup = { if (it == "slow") { slow.countDown(); await(resume) }; NetworkMonitorUpdate(it, 1) }
        val lookupWorker = thread { h.events[0].available("slow") }; await(slow)
        h.events[0].available("latest"); resume.countDown(); finish(lookupWorker)
        assertEquals("1" to "latest", h.calls.last())
        assertFalse(h.calls.any { it.second == "slow" })
    }

    @Test fun lateLostAndCapabilitiesUseExactCurrentNetwork() {
        val h = Harness(); h.session()
        h.events[0].available("wifi"); h.events[0].available("cellular")
        h.events[0].lost("wifi"); h.events[0].capabilitiesChanged("wifi")
        assertEquals("cellular", h.owner.defaultNetwork)
        h.events[0].capabilitiesChanged("cellular"); h.events[0].lost("cellular")
        assertNull(h.owner.defaultNetwork)
        assertEquals("1" to "", h.calls.last())
    }

    @Test fun closeBeforeStartAndLateStartCannotReopenNativeIncarnation() {
        val h = Harness(); val a = h.owner.createSession()
        a.closeListener(Proxy("4"))
        try { a.startListener(Proxy("4", 10)); fail() } catch (_: IllegalStateException) {}
        a.start(); a.startListener(Proxy("5")); a.startListener(Proxy("5", 20))
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
        h.events[0].available("wifi")
        assertEquals(listOf("1" to "wifi"), h.calls.toList())
    }

    @Test fun stopDuringRegistrationCompensatesExactCallbackAfterRealReturn() {
        val h = Harness(); val a = h.owner.createSession()
        val entered = CountDownLatch(1); val release = CountDownLatch(1)
        h.registration = { entered.countDown(); await(release) }
        val worker = thread { a.start() }; await(entered)
        a.stop(); assertTrue(a.snapshot().registering); assertEquals(0, h.unregistered)
        val b = h.owner.createSession()
        h.registration = {}; b.start(); b.startListener(Proxy("2")); h.events[1].available("new")
        release.countDown(); finish(worker)
        assertEquals(1, h.unregistered)
        assertEquals("new", h.owner.defaultNetwork)
        assertFalse(a.snapshot().registering)
    }

    @Test fun registrationFailureKeepsFirstErrorAndLateCallbackCannotPublish() {
        val h = Harness(); val a = h.owner.createSession()
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
        h.events[0].available("self")
        assertEquals(0, a.snapshot().delivering); assertEquals(0, a.snapshot().lookups)
        val b = h.session("2")
        val first = IllegalStateException("jni")
        h.delivery = { _, update -> if (update.name == "first") {
            h.events[1].available("latest"); throw first
        } }
        try { h.events[1].available("first"); fail() } catch (error: Throwable) { assertSame(first, error) }
        assertEquals("2" to "latest", h.calls.last())
        assertSame(first, b.snapshot().firstFailure)
        assertEquals(0, b.snapshot().delivering)
    }

    @Test fun actualAttemptRevocationFencesMonitorWhileOperationLockAndJniAreBusy() {
        val entered = CountDownLatch(1); val release = CountDownLatch(1)
        val coordinator = NetworkMonitorCoordinator<android.net.Network, io.nekohasekai.libbox.InterfaceUpdateListener>(
            registration = { object : NetworkMonitorRegistration {
                override fun register() {}
                override fun unregister() {}
            } },
            initialNetwork = { null }, lookup = { error("unexpected SDK lookup") },
            identity = { "100" }, deliver = { _, _ -> entered.countDown(); await(release) },
        )
        val session = coordinator.createSession(); session.start()
        val attempt = MainKernelAttempt<Any>(network = session)
        val listener = object : io.nekohasekai.libbox.InterfaceUpdateListener {
            override fun updateDefaultInterface(name: String, index: Int, expensive: Boolean, constrained: Boolean) {}
            override fun updateNetworkPath(path: String) {}
        }
        val worker = thread { synchronized(attempt.operationLock) { session.startListener(listener) } }
        await(entered)
        attempt.revokeAndDetachTun()
        assertTrue(attempt.revoked)
        assertTrue(session.snapshot().revoked)
        assertEquals(1, session.snapshot().delivering)
        release.countDown(); finish(worker)
        assertEquals(0, session.snapshot().delivering)
        assertEquals(0, session.snapshot().lookups)
    }
}
