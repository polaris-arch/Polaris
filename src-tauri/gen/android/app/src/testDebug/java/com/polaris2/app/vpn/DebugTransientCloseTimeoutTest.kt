package com.polaris2.app.vpn

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.*
import org.junit.Test

class DebugTransientCloseTimeoutTest {
    private fun await(latch: CountDownLatch) = assertTrue(latch.await(2, TimeUnit.SECONDS))
    private fun daemon(action: () -> Unit) = Thread(action).apply { isDaemon = true; start() }
    private val speedId = "a".repeat(32) + ":0000000000000001"
    private val absentToken = "0".repeat(16)
    private fun armCurrent(injection: DebugTransientCloseTimeout, kind: String, id: String) {
        val current = injection.currentTargets().single { it.first == kind && it.second == id }
        injection.arm(current.first, current.second, current.third)
    }

    @Test fun discoveryExposesOnlyCurrentlyStartedMinimalMetadataAndDoesNotArm() {
        val injection = DebugTransientCloseTimeout()
        assertTrue(injection.currentTargets().isEmpty())
        val speedtest = injection.Target("speedtest", speedId)
        val login = injection.Target("login", "tailscale-login-fixture-1")
        assertTrue(injection.currentTargets().isEmpty())
        speedtest.started(); login.started()
        val snapshot = injection.currentTargets()
        assertEquals(listOf("login" to "tailscale-login-fixture-1", "speedtest" to speedId), snapshot.map { it.first to it.second })
        assertEquals(2, snapshot.map { it.third }.distinct().size)
        assertEquals(snapshot, injection.currentTargets())
        speedtest.beforeClose() // Discovery has not armed either close.
        assertEquals(listOf(snapshot.first()), injection.currentTargets())
        login.beforeClose()
        assertTrue(injection.currentTargets().isEmpty())
        assertEquals(2, snapshot.size) // A copied snapshot cannot change with the registry.
    }

    @Test fun discoveryThenArmUsesTheReturnedPrivateSpeedtestIdentity() {
        val injection = DebugTransientCloseTimeout()
        val privateId = java.util.UUID.randomUUID().toString().replace("-", "") + ":0000000000000001"
        val target = injection.Target("speedtest", privateId)
        target.started()
        val (kind, id, token) = injection.currentTargets().single()
        injection.arm(kind, id, token)
        assertTrue(runCatching { injection.arm(kind, id, token) }.isFailure)
        val worker = daemon { target.beforeClose() }
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2)
        while (worker.state != Thread.State.WAITING && System.nanoTime() < deadline) Thread.yield()
        assertEquals(Thread.State.WAITING, worker.state)
        assertTrue(injection.currentTargets().isEmpty())
    }

    @Test fun discoveryIsBoundedByTheActualRegistryAndClosingReleasesOnlyMetadata() {
        val injection = DebugTransientCloseTimeout()
        val targets = (1..9).map { injection.Target("login", "fixture-$it").also { it.started() } }
        assertEquals(9, injection.currentTargets().size)
        assertTrue(runCatching { injection.Target("login", "extra").started() }.isFailure)
        targets.first().beforeClose()
        assertEquals(8, injection.currentTargets().size)
        assertFalse(injection.currentTargets().any { it.first == "login" && it.second == "fixture-1" })
    }

    @Test fun closeRacingDiscoveryCannotLeaveAStaleArmableTarget() {
        repeat(16) {
            val injection = DebugTransientCloseTimeout()
            val target = injection.Target("login", "fixture-$it")
            target.started()
            val snapshot = injection.currentTargets()
            val closed = CountDownLatch(1)
            daemon { target.beforeClose(); closed.countDown() }
            repeat(8) {
                val current = injection.currentTargets()
                assertTrue(current.isEmpty() || current == snapshot)
            }
            await(closed)
            assertTrue(injection.currentTargets().isEmpty())
            val (kind, id, token) = snapshot.single()
            assertTrue(runCatching { injection.arm(kind, id, token) }.isFailure)
        }
    }

    @Test fun strictInputsAndMissingTargetsDoNotArmFutureEngines() {
        val injection = DebugTransientCloseTimeout()
        for ((kind, id) in listOf("main" to speedId, "login" to "", "login" to "x".repeat(257),
            "login" to "a/b", "login" to "a\nb", "login" to "中文", "LOGIN" to "id")) {
            assertTrue(runCatching { injection.arm(kind, id, absentToken) }.isFailure)
        }
        assertTrue(runCatching { injection.arm("login", "missing", absentToken) }.isFailure)
        val target = injection.Target("login", "missing")
        target.started()
        target.beforeClose() // The earlier rejected arm did not poison this instance.
        assertTrue(runCatching { injection.arm("login", "missing", absentToken) }.isFailure)
    }

    @Test fun closeOvertakingStartCannotBecomeAnArmableRunningEngine() {
        val injection = DebugTransientCloseTimeout()
        val target = injection.Target("speedtest", speedId)
        target.beforeClose()
        target.started()
        assertTrue(injection.currentTargets().isEmpty())
        assertTrue(runCatching { injection.arm("speedtest", speedId, absentToken) }.isFailure)
    }

    @Test fun staleSnapshotAndCloseCannotTargetSuccessorOrConsumeTheArm() {
        val injection = DebugTransientCloseTimeout()
        val old = injection.Target("login", "same-id")
        old.started()
        val stale = injection.currentTargets().single()
        old.beforeClose()
        val successor = injection.Target("login", "same-id")
        successor.started()
        old.beforeClose()
        val (kind, id, token) = injection.currentTargets().single()
        assertEquals("login" to "same-id", kind to id)
        assertNotEquals(stale.third, token)
        assertTrue(runCatching { injection.arm(stale.first, stale.second, stale.third) }.isFailure)
        val other = injection.Target("speedtest", "same-id")
        other.started()
        assertTrue(runCatching { injection.arm("speedtest", "same-id", token) }.isFailure)
        other.beforeClose()
        for (bad in listOf("", "0".repeat(15), "0".repeat(17), "g".repeat(16), "A".repeat(16), absentToken)) {
            assertTrue(runCatching { injection.arm(kind, id, bad) }.isFailure)
        }
        injection.arm(kind, id, token) // Every stale/wrong token above left the one arm unused.
        val returned = CountDownLatch(1)
        daemon { successor.beforeClose(); returned.countDown() }
        assertFalse(returned.await(60, TimeUnit.MILLISECONDS))
        assertTrue(runCatching { injection.arm("login", "same-id", token) }.isFailure)
    }

    @Test fun armedCloseStaysBlockedAfterInterruptAndRejectsFurtherArms() {
        val injection = DebugTransientCloseTimeout()
        val target = injection.Target("login", "tailscale-login-fixture-1")
        target.started()
        armCurrent(injection, "login", "tailscale-login-fixture-1")
        assertTrue(runCatching { armCurrent(injection, "login", "tailscale-login-fixture-1") }.isFailure)
        val returned = CountDownLatch(1)
        val worker = daemon { target.beforeClose(); returned.countDown() }
        worker.interrupt()
        assertFalse(returned.await(60, TimeUnit.MILLISECONDS))
        val other = injection.Target("login", "tailscale-login-fixture-2")
        other.started(); other.beforeClose()
        assertTrue(runCatching { injection.arm("login", "tailscale-login-fixture-2", absentToken) }.isFailure)
    }

    @Test fun armedOldEngineCannotBlockReusedIdOrAnotherType() {
        val injection = DebugTransientCloseTimeout()
        val old = injection.Target("login", "same-id")
        old.started(); armCurrent(injection, "login", "same-id")
        val worker = daemon { old.beforeClose() }
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2)
        while (worker.state != Thread.State.WAITING && System.nanoTime() < deadline) Thread.yield()
        assertEquals(Thread.State.WAITING, worker.state)
        val successor = injection.Target("login", "same-id")
        successor.started(); successor.beforeClose()
        val otherType = injection.Target("speedtest", "same-id")
        otherType.started(); otherType.beforeClose()
        assertEquals(Thread.State.WAITING, worker.state)
    }

    @Test fun mainPreemptionItselfEntersArmedSpeedtestCleanupAndRejectsMain() {
        val injection = DebugTransientCloseTimeout()
        val sessions = TransientSpeedtestSessions(closeTimeoutMillis = 40)
        val target = injection.Target("speedtest", speedId)
        val closeEntered = CountDownLatch(1)
        val cleanup = cleanup { closeEntered.countDown(); target.beforeClose() }
        val started = CountDownLatch(1)
        sessions.start(speedId, object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() { target.started() }
            override fun close() = cleanup.close()
        }) { assertNull(it); started.countDown() }
        await(started); armCurrent(injection, "speedtest", speedId)
        val rejected = CountDownLatch(1)
        val outcome = AtomicReference<Throwable?>()
        val mainCalls = AtomicInteger()
        daemon {
            outcome.set(runCatching { sessions.withMainStart(Any(), { true }) { mainCalls.incrementAndGet() } }.exceptionOrNull())
            rejected.countDown()
        }
        await(closeEntered); await(rejected)
        assertTrue(outcome.get()?.message.orEmpty().contains("cleanupUnknown"))
        assertEquals(0, mainCalls.get())
    }

    @Test fun speedtestRealCloseTimesOutAndMainActionIsRejectedWithoutSettlingOwner() {
        val injection = DebugTransientCloseTimeout()
        val ledger = ledger()
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger, closeTimeoutMillis = 40)
        val target = injection.Target("speedtest", speedId)
        val closeEntered = CountDownLatch(1)
        val nativeCloses = AtomicInteger()
        val cleanup = cleanup { closeEntered.countDown(); target.beforeClose(); nativeCloses.incrementAndGet() }
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() { target.started() }
            override fun close() = cleanup.close()
        }
        val ticket = sessions.reserveOwner(speedId)
        val started = CountDownLatch(1)
        sessions.start(speedId, engine, ticket) { assertNull(it); started.countDown() }
        await(started)
        val (kind, currentId, token) = injection.currentTargets().single()
        injection.arm(kind, currentId, token)
        val replied = CountDownLatch(1)
        val failure = AtomicReference<String?>()
        daemon { sessions.close(speedId) { failure.set(it); replied.countDown() } }
        await(closeEntered); await(replied)
        assertNotNull(failure.get())
        assertEquals("cleanupUnknown", sessions.status(speedId))
        val mains = AtomicInteger()
        assertTrue(runCatching { sessions.withMainStart(Any(), { true }) { mains.incrementAndGet() } }.isFailure)
        assertEquals(0, mains.get())
        assertEquals(0, nativeCloses.get())
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(ticket))
        // A new process has new host/ledger/injection state; this is a JVM fixture, not an App restart observation.
        assertEquals(1, TransientSpeedtestSessions(nativeAdmission = ledger()).withMainStart(Any(), { true }) { 1 })
    }

    @Test fun unarmedSpeedtestUsesExistingCleanupAndAllowsMain() {
        val injection = DebugTransientCloseTimeout()
        val sessions = TransientSpeedtestSessions(closeTimeoutMillis = 300)
        val target = injection.Target("speedtest", speedId)
        val closes = AtomicInteger()
        val cleanup = cleanup { target.beforeClose(); closes.incrementAndGet() }
        val started = CountDownLatch(1)
        sessions.start(speedId, object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() { target.started() }
            override fun close() = cleanup.close()
        }) { assertNull(it); started.countDown() }
        await(started)
        assertEquals("main", sessions.withMainStart(Any(), { true }) { "main" })
        assertEquals(1, closes.get())
        assertEquals("closed", sessions.status(speedId))
    }

    @Test fun loginRealCloseRetainsReceiptAndPreventsConflictingMainEntry() {
        val injection = DebugTransientCloseTimeout()
        val ledger = ledger()
        val target = injection.Target("login", "tailscale-login-fixture-1")
        val closeEntered = CountDownLatch(1)
        val nativeCloses = AtomicInteger()
        val cleanup = cleanup { closeEntered.countDown(); target.beforeClose(); nativeCloses.incrementAndGet() }
        val host = login(ledger, target, cleanup)
        val started = CountDownLatch(1)
        host.start("tailscale-login-fixture-1", "synthetic") { assertNull(it); started.countDown() }
        await(started)
        armCurrent(injection, "login", "tailscale-login-fixture-1")
        val replied = CountDownLatch(1)
        host.close("tailscale-login-fixture-1") { replied.countDown() }
        await(closeEntered)
        val enteredMain = CountDownLatch(1)
        daemon { host.withMainConfig(MainKernelAttempt<Any>(), "synthetic") { enteredMain.countDown() } }
        assertFalse(replied.await(60, TimeUnit.MILLISECONDS))
        assertFalse(enteredMain.await(60, TimeUnit.MILLISECONDS))
        assertEquals(0, nativeCloses.get())
        assertTrue(host.running("tailscale-login-fixture-1"))
        val ticket = ledger.seal("fixture-fence").captured.single { it.ticket.kind == AndroidNativeAdmission.Kind.Login }.ticket
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(ticket))
    }

    @Test fun unarmedLoginUsesExistingCleanupAndAllowsConflictingMain() {
        val injection = DebugTransientCloseTimeout()
        val target = injection.Target("login", "tailscale-login-fixture-1")
        val closes = AtomicInteger()
        val host = login(ledger(), target, cleanup { target.beforeClose(); closes.incrementAndGet() })
        val started = CountDownLatch(1)
        host.start("tailscale-login-fixture-1", "synthetic") { assertNull(it); started.countDown() }
        await(started)
        assertEquals("main", host.withMainConfig(MainKernelAttempt<Any>(), "synthetic") { "main" })
        assertEquals(1, closes.get())
        assertFalse(host.running("tailscale-login-fixture-1"))
    }

    private fun ledger() = AndroidNativeAdmission("fixture-process").also { it.bootstrap(RequiredMarkerProof.Absent) }
    private fun cleanup(closeService: () -> Unit) = TransientHostCleanup({}, closeService, {}, {})
    private fun login(ledger: AndroidNativeAdmission, target: DebugTransientCloseTimeout.Target,
        cleanup: TransientHostCleanup) = TransientLoginHostState(
        ledger = ledger,
        queue = { daemon(it); Unit },
        schedule = { _, _ -> },
        parseDirectories = { setOf("/synthetic/tailscale/state") },
        requireSupported = {},
        createEngine = { _, _, _, _ -> object : TransientLoginHostState.Engine {
            override fun prepare(ticket: AndroidNativeAdmission.Ticket, stage: (String) -> Unit, cancelled: () -> Boolean) {
                AndroidNativeValidation.run(ledger, ticket, setup = {}, nativeCheck = {})
            }
            override fun start() { target.started() }
            override fun close() = cleanup.close()
        } },
    )
}
