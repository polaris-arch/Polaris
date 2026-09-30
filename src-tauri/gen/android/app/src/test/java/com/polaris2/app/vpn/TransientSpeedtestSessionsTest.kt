package com.polaris2.app.vpn

import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.CancellationException
import java.util.concurrent.Executor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TransientSpeedtestSessionsTest {
    private class ResolverEngine : TransientSpeedtestSessions.Engine {
        private val cancelQueue = java.util.concurrent.LinkedBlockingQueue<Runnable>()
        val resolver = TransientResolverLifecycle(Executor { cancelQueue.add(it) })
        val stages = mutableListOf<String>()
        val closes = AtomicInteger()
        var serviceAction: () -> Unit = {}
        var serviceFailure: Throwable? = null
        var serverFailure: Throwable? = null
        private var proof = true
        private val cleanup = TransientHostCleanup(
            beginResolverClose = { stages.add("fence"); resolver.beginClose() },
            closeService = { stages.add("service"); serviceAction(); serviceFailure?.let { throw it } },
            closeServer = { stages.add("server"); serverFailure?.let { throw it } },
            closeNetwork = { stages.add("network"); resolver.closeUnused() },
            resolverUnknown = { proof = false },
        )
        override fun prepare() {}
        override fun start() {}
        override fun close() { closes.incrementAndGet(); cleanup.close() }
        override fun cleanupConfirmed(): Boolean = proof
        fun runCancel() { checkNotNull(cancelQueue.poll()).run() }
        fun pendingCancels(): Int = cancelQueue.size
    }

    @Test fun productionCleanupFencesBeforeNativeJoinAndReentrantCloseWithoutWaitingForSdkCancel() {
        val ledger = ledger()
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger)
        val engine = ResolverEngine()
        val ticket = sessions.reserveOwner(id(1))
        val (started, startResult) = startNative(sessions, 1, engine, ticket)
        await(started)
        assertEquals(null, startResult.get())
        val query = engine.resolver.enterQuery<Int>()
        val queryDone = CountDownLatch(1)
        val reenteredDone = CountDownLatch(1)
        val queryError = AtomicReference<Throwable?>()
        val writes = AtomicInteger()
        val signalCalls = AtomicInteger()
        query.installCancellation { signalCalls.incrementAndGet() }
        Thread {
            try { query.awaitAndDeliver { writes.incrementAndGet() } }
            catch (_: CancellationException) {}
            catch (failure: Throwable) { queryError.set(failure) }
            finally { query.returned(); queryDone.countDown() }
        }.start()
        engine.serviceAction = {
            assertTrue(engine.resolver.snapshot().sealed)
            assertTrue(runCatching { engine.resolver.enterQuery<Int>() }.isFailure)
            await(queryDone)
            sessions.close(id(1)) { assertEquals(null, it); reenteredDone.countDown() }
        }
        val (closed, closeResult) = close(sessions, id(1))
        await(closed)
        await(reenteredDone)
        assertEquals(null, closeResult.get())
        assertEquals(null, queryError.get())
        assertEquals(listOf("fence", "service", "server", "network"), engine.stages)
        assertEquals(0, writes.get())
        assertEquals(0, signalCalls.get())
        assertEquals(1, engine.pendingCancels())
        assertFalse(engine.cleanupConfirmed())
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
        assertEquals("closed", sessions.status(id(1)))
        val next = ResolverEngine()
        val nextTicket = sessions.reserveOwner(id(2))
        val (nextStarted, nextResult) = startNative(sessions, 2, next, nextTicket)
        await(nextStarted)
        assertEquals(null, nextResult.get())
        val (duplicate, _) = close(sessions, id(1))
        await(duplicate)
        assertFalse(query.publish(2))
        engine.runCancel()
        assertEquals(1, signalCalls.get())
        assertEquals(1, engine.closes.get())
        assertEquals(0, next.closes.get())
        assertFalse(next.resolver.snapshot().sealed)
        assertEquals("running", sessions.status(id(2)))
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
        assertTrue(TransientSpeedtestSessions.capabilities.isEmpty())
        val (nextClosed, _) = close(sessions, id(2))
        await(nextClosed)
    }

    @Test fun productionCleanupKeepsAlreadyDeliveringJniAndQueuedCancelUnknownAfterOperationalSuccess() {
        val ledger = ledger()
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger)
        val engine = ResolverEngine()
        val ticket = sessions.reserveOwner(id(1))
        val (started, _) = startNative(sessions, 1, engine, ticket)
        await(started)
        val query = engine.resolver.enterQuery<Int>()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val queryDone = CountDownLatch(1)
        val queryError = AtomicReference<Throwable?>()
        val writes = AtomicInteger()
        query.installCancellation {}
        query.publish(1)
        Thread {
            try { query.awaitAndDeliver { entered.countDown(); await(release); writes.incrementAndGet() } }
            catch (failure: Throwable) { queryError.set(failure) }
            finally { query.returned(); queryDone.countDown() }
        }.start()
        try {
            await(entered)
            val (closed, result) = close(sessions, id(1))
            await(closed)
            assertEquals(null, result.get())
            assertEquals("closed", sessions.status(id(1)))
            assertEquals(1, engine.resolver.snapshot().calls)
            assertEquals(1, engine.resolver.snapshot().deliveries)
            assertEquals(1, engine.resolver.snapshot().cancellationTasks)
            assertEquals(TransientResolverLifecycle.Drain.Unknown, engine.resolver.awaitLocalDrain(0, TimeUnit.SECONDS))
            assertEquals(1, engine.resolver.snapshot().deliveries)
            assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
        } finally { release.countDown() }
        await(queryDone)
        assertEquals(null, queryError.get())
        assertEquals(1, writes.get())
        assertEquals(0, engine.resolver.snapshot().deliveries)
        assertEquals(1, engine.resolver.snapshot().cancellationTasks)
        engine.runCancel()
        assertEquals(0, engine.resolver.snapshot().cancellationTasks)
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
    }

    @Test fun realNativeCloseFailureWinsOverQueriedResolverProofAndRetainsOperationalOwner() {
        val ledger = ledger()
        val logged = AtomicReference<Throwable?>()
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger, logFailure = { logged.set(it) })
        val engine = ResolverEngine()
        val actual = IllegalStateException("real CloseService failure")
        engine.serviceFailure = actual
        engine.serverFailure = IllegalStateException("secondary Close failure")
        val ticket = sessions.reserveOwner(id(1))
        val (started, _) = startNative(sessions, 1, engine, ticket)
        await(started)
        val query = engine.resolver.enterQuery<Int>()
        val (closed, result) = close(sessions, id(1))
        await(closed)
        assertTrue(result.get()!!.contains("未知"))
        assertTrue(logged.get() === actual)
        assertEquals(listOf("fence", "service", "server", "network"), engine.stages)
        assertFalse(engine.cleanupConfirmed())
        assertEquals("cleanupUnknown", sessions.status(id(1)))
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
        assertFalse(query.publish(1))
        query.returned()
        val untouched = AtomicInteger()
        assertTrue(runCatching { sessions.withMainStart(Any(), { true }) { untouched.incrementAndGet() } }.isFailure)
        assertEquals(0, untouched.get())
    }

    @Test fun queuedSpeedtestGetsExplicitCapacityCauseWithoutNativeBirth() {
        val ledger = AndroidNativeAdmission("speed-capacity", maxMetadataRecords = 3).also { it.bootstrap(RequiredMarkerProof.Absent) }
        var worker: (() -> Unit)? = null
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger, launchStart = { worker = it })
        val ticket = sessions.reserveOwner(id(1))
        val nativeCalls = AtomicInteger()
        val done = CountDownLatch(1)
        val result = AtomicReference<AndroidNativeFailure?>()
        sessions.startCoded(id(1), object : TransientSpeedtestSessions.Engine {
            override fun prepare() { nativeCalls.incrementAndGet() }
            override fun start() { nativeCalls.incrementAndGet() }
            override fun close() {}
        }, ticket) { result.set(it); done.countDown() }
        ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "fill")
        worker!!.invoke()
        await(done)
        assertEquals(AndroidNativeAdmission.CAPACITY_CODE, result.get()!!.code)
        assertEquals(0, nativeCalls.get())
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(ticket))
        val unknown = AtomicReference<AndroidNativeFailure?>()
        sessions.closeCoded(id(2)) { unknown.set(it) }
        assertEquals(AndroidNativeAdmission.CAPACITY_CODE, unknown.get()!!.code)
    }

    @Test fun enteredSpeedtestCloseFailureAtCapacityStillMeansCleanupUnknown() {
        val ledger = AndroidNativeAdmission("speed-capacity", maxMetadataRecords = 3).also { it.bootstrap(RequiredMarkerProof.Absent) }
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger)
        val ticket = sessions.reserveOwner(id(1))
        val started = CountDownLatch(1)
        sessions.startCoded(id(1), object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() {}
            override fun close() { error(AndroidNativeAdmission.CAPACITY_MESSAGE) }
        }, ticket) { assertEquals(null, it); started.countDown() }
        await(started)
        ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "fill")
        val closed = CountDownLatch(1)
        val result = AtomicReference<AndroidNativeFailure?>()
        sessions.closeCoded(id(1)) { result.set(it); closed.countDown() }
        await(closed)
        assertEquals(null, result.get()!!.code)
        assertEquals("cleanupUnknown", sessions.status(id(1)))
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
    }
    private fun await(latch: CountDownLatch) = assertTrue(latch.await(2, TimeUnit.SECONDS))
    private fun id(sequence: Int, epoch: String = "a".repeat(32)): String =
        "$epoch:${sequence.toString(16).padStart(16, '0')}"

    private fun start(
        sessions: TransientSpeedtestSessions,
        id: String,
        engine: TransientSpeedtestSessions.Engine,
    ): Pair<CountDownLatch, AtomicReference<String?>> {
        val callback = CountDownLatch(1)
        val result = AtomicReference<String?>()
        sessions.start(id, engine) { result.set(it); callback.countDown() }
        return callback to result
    }

    private fun close(sessions: TransientSpeedtestSessions, id: String): Pair<CountDownLatch, AtomicReference<String?>> {
        val callback = CountDownLatch(1)
        val result = AtomicReference<String?>()
        sessions.close(id) { result.set(it); callback.countDown() }
        return callback to result
    }

    @Test fun closeRunsWhileNativeStartIsBlockedAndCannotReviveSession() {
        val sessions = TransientSpeedtestSessions(closeTimeoutMillis = 500)
        val inStart = CountDownLatch(1)
        val releaseStart = CountDownLatch(1)
        val closeCalls = AtomicInteger()
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() { inStart.countDown(); await(releaseStart) }
            override fun close() { closeCalls.incrementAndGet(); releaseStart.countDown() }
        }
        val (started, startResult) = start(sessions, id(1), engine)
        await(inStart)
        val (closed, closeResult) = close(sessions, id(1))
        await(closed)
        await(started)
        assertEquals(null, closeResult.get())
        assertEquals("Android 临时测速已取消", startResult.get())
        assertEquals("closed", sessions.status(id(1)))
        assertEquals(1, closeCalls.get())
    }

    @Test fun mainStartWaitsForActualCloseAndReservesAdmission() {
        val sessions = TransientSpeedtestSessions(closeTimeoutMillis = 500)
        val closeEntered = CountDownLatch(1)
        val releaseClose = CountDownLatch(1)
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() {}
            override fun close() { closeEntered.countDown(); await(releaseClose) }
        }
        val (started, _) = start(sessions, id(1), engine)
        await(started)
        val mainOwner = Any()
        val mainEntered = CountDownLatch(1)
        val mainDone = CountDownLatch(1)
        Thread {
            sessions.withMainStart(mainOwner, { true }) { mainEntered.countDown() }
            mainDone.countDown()
        }.start()
        await(closeEntered)
        assertFalse(mainEntered.await(30, TimeUnit.MILLISECONDS))
        val (blocked, busy) = start(sessions, id(2), engine)
        await(blocked)
        assertTrue(busy.get()!!.contains("忙"))
        releaseClose.countDown()
        await(mainDone)
        assertEquals("closed", sessions.status(id(1)))
        sessions.closeMain(mainOwner) {}
    }

    @Test fun timeoutKeepsOwnerAndLateSuccessDoesNotRestartMainOrCloseNextEntry() {
        val sessions = TransientSpeedtestSessions(closeTimeoutMillis = 60)
        val closeEntered = CountDownLatch(1)
        val releaseClose = CountDownLatch(1)
        val oldCloseCalls = AtomicInteger()
        val old = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() {}
            override fun close() { oldCloseCalls.incrementAndGet(); closeEntered.countDown(); await(releaseClose) }
        }
        val (started, _) = start(sessions, id(1), old)
        await(started)
        val (closed, closeResult) = close(sessions, id(1))
        await(closeEntered)
        await(closed)
        assertTrue(closeResult.get()!!.contains("未知"))
        assertEquals("cleanupUnknown", sessions.status(id(1)))
        val mainEntered = AtomicInteger()
        val owner = Any()
        val rejected = runCatching { sessions.withMainStart(owner, { true }) { mainEntered.incrementAndGet() } }
        assertTrue(rejected.isFailure)
        assertEquals(0, mainEntered.get())
        val (blocked, _) = start(sessions, id(2), old)
        await(blocked)
        assertEquals("closed", sessions.status(id(2)))
        releaseClose.countDown()
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2)
        while (sessions.status(id(1)) != "closed" && System.nanoTime() < deadline) Thread.yield()
        assertEquals("closed", sessions.status(id(1)))
        assertEquals(0, mainEntered.get())
        val newCloseCalls = AtomicInteger()
        val next = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() {}
            override fun close() { newCloseCalls.incrementAndGet() }
        }
        val (nextStarted, _) = start(sessions, id(3), next)
        await(nextStarted)
        assertEquals("running", sessions.status(id(3)))
        assertEquals(1, oldCloseCalls.get())
        assertEquals(0, newCloseCalls.get())
        val (nextClosed, _) = close(sessions, id(3))
        await(nextClosed)
        assertEquals(1, newCloseCalls.get())
    }

    @Test fun nativeCloseFailureStaysStickyAndMainStartIsRejected() {
        val sessions = TransientSpeedtestSessions(closeTimeoutMillis = 200)
        val closeCalls = AtomicInteger()
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() {}
            override fun close() { closeCalls.incrementAndGet(); error("native close failed") }
        }
        val (started, _) = start(sessions, id(1), engine)
        await(started)
        val (closed, result) = close(sessions, id(1))
        await(closed)
        assertTrue(result.get()!!.contains("未知"))
        assertEquals("cleanupUnknown", sessions.status(id(1)))
        val (again, second) = close(sessions, id(1))
        await(again)
        assertTrue(second.get()!!.contains("未知"))
        assertEquals(1, closeCalls.get())
        var entered = false
        assertTrue(runCatching { sessions.withMainStart(Any(), { true }) { entered = true } }.isFailure)
        assertFalse(entered)
    }

    @Test fun closeDuringFactoryWaitsAndSkipsNativeStart() {
        val sessions = TransientSpeedtestSessions(closeTimeoutMillis = 60)
        val preparing = CountDownLatch(1)
        val releasePrepare = CountDownLatch(1)
        val starts = AtomicInteger()
        val closes = AtomicInteger()
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { preparing.countDown(); await(releasePrepare) }
            override fun start() { starts.incrementAndGet() }
            override fun close() { closes.incrementAndGet() }
        }
        val (started, _) = start(sessions, id(1), engine)
        await(preparing)
        val (closed, result) = close(sessions, id(1))
        await(closed)
        assertTrue(result.get()!!.contains("未知"))
        releasePrepare.countDown()
        await(started)
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2)
        while (sessions.status(id(1)) != "closed" && System.nanoTime() < deadline) Thread.yield()
        assertEquals("closed", sessions.status(id(1)))
        assertEquals(0, starts.get())
        assertEquals(1, closes.get())
    }

    @Test fun failedStartWithConfirmedCleanupAllowsRetryAndWrongIdCannotCloseIt() {
        val sessions = TransientSpeedtestSessions(closeTimeoutMillis = 300)
        val failed = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() { error("start failed") }
            override fun close() {}
        }
        val (started, result) = start(sessions, id(1), failed)
        await(started)
        assertTrue(result.get()!!.contains("启动失败"))
        val (closed, _) = close(sessions, id(1))
        await(closed)
        val good = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() {}
            override fun close() {}
        }
        val (retried, retryResult) = start(sessions, id(2), good)
        await(retried)
        assertEquals(null, retryResult.get())
        val (wrong, wrongResult) = close(sessions, id(3))
        await(wrong)
        assertEquals(null, wrongResult.get())
        assertEquals("closed", sessions.status(id(3)))
        assertEquals("running", sessions.status(id(2)))
        val (end, _) = close(sessions, id(2))
        await(end)
    }

    @Test fun closeBeforeDelayedStartRevokesIdWithoutConstructingNativeEngine() {
        val sessions = TransientSpeedtestSessions()
        assertEquals("unknown", sessions.status(id(1)))
        val (closed, closeResult) = close(sessions, id(1))
        await(closed)
        assertEquals(null, closeResult.get())
        assertEquals("closed", sessions.status(id(1)))
        val touched = AtomicInteger()
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { touched.incrementAndGet() }
            override fun start() { touched.incrementAndGet() }
            override fun close() { touched.incrementAndGet() }
        }
        val (started, result) = start(sessions, id(1), engine)
        await(started)
        assertTrue(result.get()!!.contains("忙"))
        assertEquals(0, touched.get())
    }

    @Test fun failedMainCloseRetainsClaimUntilConfirmedRetry() {
        val sessions = TransientSpeedtestSessions()
        val owner = Any()
        val anotherOwner = Any()
        sessions.withMainStart(owner, { true }) {}
        assertTrue(runCatching { sessions.closeMain(owner) { error("native main close failed") } }.isFailure)
        assertTrue(runCatching { sessions.withMainStart(anotherOwner, { true }) {} }.isFailure)
        val untouched = AtomicInteger()
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { untouched.incrementAndGet() }
            override fun start() { untouched.incrementAndGet() }
            override fun close() { untouched.incrementAndGet() }
        }
        val (blocked, _) = start(sessions, id(1), engine)
        await(blocked)
        assertEquals(0, untouched.get())
        sessions.closeMain(owner) {}
        sessions.withMainStart(anotherOwner, { true }) {}
        sessions.closeMain(anotherOwner) {}
        val (admitted, _) = start(sessions, id(2), engine)
        await(admitted)
        assertEquals("running", sessions.status(id(2)))
        val (closed, _) = close(sessions, id(2))
        await(closed)
    }

    @Test fun futureCloseAndWrongEpochCannotCloseActiveSession() {
        val sessions = TransientSpeedtestSessions()
        val closeCalls = AtomicInteger()
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() {}
            override fun close() { closeCalls.incrementAndGet() }
        }
        val (started, _) = start(sessions, id(1), engine)
        await(started)
        val (future, futureResult) = close(sessions, id(3))
        await(future)
        assertEquals(null, futureResult.get())
        assertEquals("running", sessions.status(id(1)))
        assertEquals("closed", sessions.status(id(2)))
        assertEquals(0, closeCalls.get())
        val otherEpoch = "b".repeat(32)
        val (wrong, wrongResult) = close(sessions, id(4, otherEpoch))
        await(wrong)
        assertTrue(wrongResult.get()!!.contains("epoch"))
        assertEquals("running", sessions.status(id(1)))
        val (oldClosed, _) = close(sessions, id(1))
        await(oldClosed)
        assertEquals(1, closeCalls.get())
        val (next, nextResult) = start(sessions, id(4), engine)
        await(next)
        assertEquals(null, nextResult.get())
        val (end, _) = close(sessions, id(4))
        await(end)
    }

    @Test fun busyRejectedIdCannotBeReplayedAfterOwnerCloses() {
        val sessions = TransientSpeedtestSessions()
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() {}
            override fun close() {}
        }
        val (first, _) = start(sessions, id(1), engine)
        await(first)
        val (busy, busyResult) = start(sessions, id(2), engine)
        await(busy)
        assertTrue(busyResult.get()!!.contains("忙"))
        val (firstClosed, _) = close(sessions, id(1))
        await(firstClosed)
        val (replayed, replayResult) = start(sessions, id(2), engine)
        await(replayed)
        assertTrue(replayResult.get()!!.contains("忙"))
        assertEquals("closed", sessions.status(id(2)))
        val (third, thirdResult) = start(sessions, id(3), engine)
        await(third)
        assertEquals(null, thirdResult.get())
        val (thirdClosed, _) = close(sessions, id(3))
        await(thirdClosed)
    }

    @Test fun thousandsOfCompletedRoundsAndUnknownClosesKeepAdmissionOpen() {
        val sessions = TransientSpeedtestSessions(closeTimeoutMillis = 2_000)
        val starts = AtomicInteger()
        val closes = AtomicInteger()
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() { starts.incrementAndGet() }
            override fun close() { closes.incrementAndGet() }
        }
        for (sequence in 1..2_050) {
            val (started, result) = start(sessions, id(sequence), engine)
            await(started)
            assertEquals(null, result.get())
            val (closed, closeResult) = close(sessions, id(sequence))
            await(closed)
            assertEquals(null, closeResult.get())
        }
        assertEquals(2_050, starts.get())
        assertEquals(2_050, closes.get())
        for (sequence in 2_051..12_050) {
            val (closed, result) = close(sessions, id(sequence))
            await(closed)
            assertEquals(null, result.get())
        }
        val (next, result) = start(sessions, id(12_051), engine)
        await(next)
        assertEquals(null, result.get())
        val (end, _) = close(sessions, id(12_051))
        await(end)
        assertEquals(2_051, starts.get())
        assertEquals(2_051, closes.get())
    }

    @Test fun reloadLosesStartRaceButCannotReleaseExistingMainClaim() {
        val sessions = TransientSpeedtestSessions()
        val owner = Any()
        sessions.withMainStart(owner, { true }) {}
        val checks = AtomicInteger()
        assertTrue(runCatching {
            sessions.withMainStart(owner, { checks.getAndIncrement() == 0 }) {}
        }.isFailure)
        assertEquals(2, checks.get())
        assertTrue(runCatching { sessions.withMainStart(Any(), { true }) {} }.isFailure)
        val untouched = AtomicInteger()
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { untouched.incrementAndGet() }
            override fun start() { untouched.incrementAndGet() }
            override fun close() { untouched.incrementAndGet() }
        }
        val (blocked, _) = start(sessions, id(1), engine)
        await(blocked)
        assertEquals(0, untouched.get())
        sessions.closeMain(owner) {}
        val (admitted, result) = start(sessions, id(2), engine)
        await(admitted)
        assertEquals(null, result.get())
        val (closed, _) = close(sessions, id(2))
        await(closed)
    }

    private fun ledger() = AndroidNativeAdmission("speed-process").also { it.bootstrap(RequiredMarkerProof.Absent) }

    private fun startNative(sessions: TransientSpeedtestSessions, sequence: Int,
        engine: TransientSpeedtestSessions.Engine,
        ticket: AndroidNativeAdmission.Ticket = sessions.reserveOwner(id(sequence))): Pair<CountDownLatch, AtomicReference<String?>> {
        val replied = CountDownLatch(1)
        val result = AtomicReference<String?>()
        sessions.start(id(sequence), engine, ticket) { result.set(it); replied.countDown() }
        return replied to result
    }

    @Test fun foreignTicketRejectionRepliesWithoutRetiringItsOwnerOrConsumingTheRequestedId() {
        val ledger = ledger()
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger)
        val foreignFamily = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, id(1))
        val foreignId = sessions.reserveOwner(id(2))
        val acquisitions = AtomicInteger()
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { acquisitions.incrementAndGet() }
            override fun start() { acquisitions.incrementAndGet() }
            override fun close() { }
        }
        for (foreign in listOf(foreignFamily, foreignId)) {
            val (replied, failure) = startNative(sessions, 1, engine, foreign)
            await(replied)
            assertTrue(failure.get()!!.contains("标识无效"))
            assertEquals(AndroidNativeAdmission.State.Reserved, ledger.state(foreign))
            assertEquals(0, acquisitions.get())
        }
        val owned = sessions.reserveOwner(id(1))
        val (started, result) = startNative(sessions, 1, engine, owned)
        await(started)
        assertEquals(null, result.get())
        val (closed, _) = close(sessions, id(1))
        await(closed)
        assertEquals(AndroidNativeAdmission.State.ClosedExact, ledger.state(owned))
        assertEquals(AndroidNativeAdmission.State.Reserved, ledger.state(foreignFamily))
        assertEquals(AndroidNativeAdmission.State.Reserved, ledger.state(foreignId))
    }

    @Test fun productionSpeedtestReservesBeforeFirstQueueAndUsesExactNetworkClose() {
        val directory = File("src/main/java/com/polaris2/app/vpn")
        val host = File(directory, "TransientSpeedtestHost.kt").readText()
        val plugin = File(directory, "PolarisVpnPlugin.kt").readText()
        val network = File(directory, "TransientLoginNetwork.kt").readText()
        val resolver = File(directory, "LocalResolver.kt").readText()
        val start = host.substringAfter("fun start(id: String,").substringBefore("fun close(id:")
        val reserve = start.indexOf("sessions.reserveOwner(id)")
        val queue = start.indexOf("AndroidNativeValidation.enqueue(")
        assertTrue(reserve >= 0 && reserve < queue)
        assertTrue(start.contains("LibboxEngine(id, config, validationTicket), nativeTicket"))
        assertTrue(host.contains("nativeAdmission = AndroidNativeAdmissionGate.ledger"))
        assertTrue(host.contains("TransientLoginNetwork(requireExactClose = true)"))
        assertTrue(host.contains("override fun serviceStop() { close(id) {} }"))
        assertTrue(plugin.contains("TransientSpeedtestHost.closeCoded(args.instanceId)"))
        assertTrue(network.contains("callbacks.close { closed = true; listener = null }"))
        assertTrue(network.contains("thread.join()"))
        assertTrue(network.contains("resolver.closeUnused()"))
        assertTrue(resolver.contains("query.awaitAndDeliver { it.deliver(ctx) }"))
        val sdkCallbacks = resolver.substringAfter("// These top-level callback objects")
        assertTrue(sdkCallbacks.contains("query.publish("))
        assertFalse(sdkCallbacks.contains("ctx"))
    }

    @Test fun queuedNativeOwnerIsCapturedAndSealPreventsEveryPrepareResource() {
        val ledger = ledger()
        val queued = AtomicReference<(() -> Unit)?>()
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger, launchStart = { queued.set(it) })
        val ticket = sessions.reserveOwner(id(1))
        val acquisitions = AtomicInteger()
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { acquisitions.incrementAndGet() }
            override fun start() { acquisitions.incrementAndGet() }
            override fun close() { }
        }
        val (started, result) = startNative(sessions, 1, engine, ticket)
        assertEquals(AndroidNativeAdmission.State.Reserved, ledger.seal("speed-fence").captured.single().state)
        requireNotNull(queued.get()).invoke()
        await(started)
        assertTrue(result.get()!!.contains("启动失败"))
        val (closed, failure) = close(sessions, id(1))
        await(closed)
        assertEquals(null, failure.get())
        assertEquals(0, acquisitions.get())
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(ticket))
    }

    @Test fun closeBeforeQueueTombstonesExactTicketAndEveryOlderEpochSequence() {
        val ledger = ledger()
        val queued = AtomicReference<(() -> Unit)?>()
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger, launchStart = { queued.set(it) })
        val ticket = sessions.reserveOwner(id(1))
        val untouched = AtomicInteger()
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { untouched.incrementAndGet() }
            override fun start() { untouched.incrementAndGet() }
            override fun close() { }
        }
        val (started, _) = startNative(sessions, 1, engine, ticket)
        val (closed, _) = close(sessions, id(1))
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(ticket))
        requireNotNull(queued.get()).invoke()
        await(started)
        await(closed)
        assertEquals(0, untouched.get())
        assertTrue(runCatching { sessions.reserveOwner(id(1)) }.isFailure)
        val older = sessions.reserveOwner(id(2))
        val (futureClose, _) = close(sessions, id(3))
        await(futureClose)
        val (rejected, failure) = startNative(sessions, 2, engine, older)
        await(rejected)
        assertTrue(failure.get()!!.contains("忙"))
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(older))
        assertTrue(runCatching { sessions.reserveOwner(id(3)) }.isFailure)
    }

    @Test fun normalOwnerClosesOnlyAfterNativeAndNetworkButValidationRemainsUnknown() {
        val ledger = ledger()
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger)
        val ticket = sessions.reserveOwner(id(1))
        val validation = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        val networkClosing = CountDownLatch(1)
        val releaseNetwork = CountDownLatch(1)
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {
                assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(ticket))
                AndroidNativeValidation.run(ledger, validation, setup = {}, nativeCheck = {})
            }
            override fun start() { }
            override fun close() { networkClosing.countDown(); await(releaseNetwork) }
        }
        val (started, result) = startNative(sessions, 1, engine, ticket)
        await(started)
        assertEquals(null, result.get())
        val receipt = ledger.seal("speed-fence")
        assertEquals(4, receipt.coveredProducers.size)
        assertFalse(receipt.coverageComplete)
        val (closed, closeResult) = close(sessions, id(1))
        await(networkClosing)
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(ticket))
        releaseNetwork.countDown()
        await(closed)
        assertEquals(null, closeResult.get())
        assertEquals(AndroidNativeAdmission.State.ClosedExact, ledger.state(ticket))
        assertEquals(AndroidNativeAdmission.State.ValidationCleanupUnknown, ledger.state(validation))
    }

    @Test fun failedPrepareOrBoxNewKeepsNativeUnknownDespiteSuccessfulOrdinaryClose() {
        for (failPrepare in listOf(false, true)) {
            val ledger = ledger()
            val sessions = TransientSpeedtestSessions(nativeAdmission = ledger)
            val ticket = sessions.reserveOwner(id(1))
            val closeCalls = AtomicInteger()
            val engine = object : TransientSpeedtestSessions.Engine {
                override fun prepare() { if (failPrepare) error("partial native factory") }
                override fun start() { error("partial box.New") }
                override fun close() { closeCalls.incrementAndGet() }
            }
            val (started, startResult) = startNative(sessions, 1, engine, ticket)
            await(started)
            assertTrue(startResult.get()!!.contains("失败"))
            val (closed, closeResult) = close(sessions, id(1))
            await(closed)
            assertEquals(null, closeResult.get())
            assertEquals("closed", sessions.status(id(1)))
            assertEquals(1, closeCalls.get())
            assertEquals(AndroidNativeAdmission.State.Unknown, ledger.seal("speed-fence").captured.single().state)
            val owner = Any()
            var entered = false
            sessions.withMainStart(owner, { true }) { entered = true }
            assertTrue(entered)
            sessions.closeMain(owner) { }
        }
    }

    @Test fun nativeStartCannotSettleBeforeItReturnsAndRevokedCompletionStaysUnknown() {
        val ledger = ledger()
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger, closeTimeoutMillis = 40)
        val ticket = sessions.reserveOwner(id(1))
        val inNative = CountDownLatch(1)
        val releaseNative = CountDownLatch(1)
        val nativeClosed = CountDownLatch(1)
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { }
            override fun start() { inNative.countDown(); await(releaseNative) }
            override fun close() { nativeClosed.countDown() }
        }
        val (started, _) = startNative(sessions, 1, engine, ticket)
        await(inNative)
        val (timedOut, timeoutResult) = close(sessions, id(1))
        await(nativeClosed)
        await(timedOut)
        assertTrue(timeoutResult.get()!!.contains("未知"))
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.seal("speed-fence").captured.single().state)
        val (late, lateResult) = close(sessions, id(1))
        releaseNative.countDown()
        await(started)
        await(late)
        assertEquals(null, lateResult.get())
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
        assertEquals("closed", sessions.status(id(1)))
    }

    @Test fun failedWorkerDispatchCancelsWithoutEnteringBirth() {
        val ledger = ledger()
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger, launchStart = { error("queue unavailable") })
        val ticket = sessions.reserveOwner(id(1))
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { error("must not acquire") }
            override fun start() { error("must not acquire") }
            override fun close() { }
        }
        val (started, failure) = startNative(sessions, 1, engine, ticket)
        await(started)
        assertTrue(failure.get()!!.contains("线程"))
        val (closed, result) = close(sessions, id(1))
        await(closed)
        assertEquals(null, result.get())
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(ticket))
    }

    @Test fun expiryAndMainPreemptionCloseOnlyTheirOriginalTicket() {
        for (preempt in listOf(false, true)) {
            val ledger = ledger()
            val sessions = TransientSpeedtestSessions(nativeAdmission = ledger, expiryMillis = if (preempt) 10_000 else 30)
            val ticket = sessions.reserveOwner(id(1))
            val closeEntered = CountDownLatch(1)
            val releaseClose = CountDownLatch(1)
            val mainDone = CountDownLatch(1)
            val mainFailure = AtomicReference<Throwable?>()
            val mainOwner = Any()
            val engine = object : TransientSpeedtestSessions.Engine {
                override fun prepare() { }
                override fun start() { }
                override fun close() { closeEntered.countDown(); await(releaseClose) }
            }
            val (started, _) = startNative(sessions, 1, engine, ticket)
            await(started)
            if (preempt) Thread {
                mainFailure.set(runCatching { sessions.withMainStart(mainOwner, { true }) { } }.exceptionOrNull())
                mainDone.countDown()
            }.start()
            await(closeEntered)
            assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(ticket))
            val (closed, result) = close(sessions, id(1))
            releaseClose.countDown()
            await(closed)
            assertEquals(null, result.get())
            assertEquals(AndroidNativeAdmission.State.ClosedExact, ledger.state(ticket))
            if (preempt) { await(mainDone); assertEquals(null, mainFailure.get()); sessions.closeMain(mainOwner) { } }
        }
    }

    @Test fun exactCallbackCloseJoinsNotificationsAndRejectsEveryQueuedLateCallback() {
        val callbacks = TransientNetworkCallbacks()
        val inNative = CountDownLatch(1)
        val release = CountDownLatch(1)
        val closing = CountDownLatch(1)
        val closed = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>()
        val worker = Thread {
            failure.set(runCatching { callbacks.dispatch { inNative.countDown(); await(release) } }.exceptionOrNull())
        }
        worker.start()
        await(inNative)
        val closer = Thread { closing.countDown(); callbacks.close { closed.countDown() } }
        closer.start()
        await(closing)
        assertEquals(1L, closed.count)
        release.countDown()
        await(closed)
        callbacks.dispatch { error("native callback entered after close") }
        worker.join(2_000)
        closer.join(2_000)
        assertFalse(worker.isAlive)
        assertFalse(closer.isAlive)
        assertEquals(null, failure.get())
    }

    @Test fun lateConfirmedCloseAndOldCallbackCannotSettleOrCloseNewTicket() {
        val ledger = ledger()
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger, closeTimeoutMillis = 40)
        val ticketA = sessions.reserveOwner(id(1))
        val ticketB = sessions.reserveOwner(id(2))
        val closeEntered = CountDownLatch(1)
        val releaseClose = CountDownLatch(1)
        val closesB = AtomicInteger()
        val old = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { }
            override fun start() { }
            override fun close() { closeEntered.countDown(); await(releaseClose) }
        }
        val (startedA, _) = startNative(sessions, 1, old, ticketA)
        await(startedA)
        val (timeout, timeoutResult) = close(sessions, id(1))
        await(closeEntered)
        await(timeout)
        assertTrue(timeoutResult.get()!!.contains("未知"))
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(ticketA))
        val (confirmed, confirmedResult) = close(sessions, id(1))
        releaseClose.countDown()
        await(confirmed)
        assertEquals(null, confirmedResult.get())
        val next = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { }
            override fun start() { }
            override fun close() { closesB.incrementAndGet() }
        }
        val (startedB, resultB) = startNative(sessions, 2, next, ticketB)
        await(startedB)
        assertEquals(null, resultB.get())
        ledger.seal("speed-fence")
        val (oldAgain, _) = close(sessions, id(1))
        await(oldAgain)
        assertEquals(0, closesB.get())
        assertEquals("running", sessions.status(id(2)))
        assertEquals(AndroidNativeAdmission.State.ClosedExact, ledger.state(ticketA))
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(ticketB))
        val (closedB, _) = close(sessions, id(2))
        await(closedB)
        assertEquals(AndroidNativeAdmission.State.ClosedExact, ledger.state(ticketB))
    }

    @Test fun boxNewFailureVersusStopHasNoPositiveTerminalInEitherOrder() {
        for (stopFirst in listOf(false, true)) {
            val ledger = ledger()
            val sessions = TransientSpeedtestSessions(nativeAdmission = ledger)
            val ticket = sessions.reserveOwner(id(1))
            val inNative = CountDownLatch(1)
            val leave = CountDownLatch(1)
            val closeEntered = CountDownLatch(1)
            val engine = object : TransientSpeedtestSessions.Engine {
                override fun prepare() { }
                override fun start() { inNative.countDown(); await(leave); error("partial box.New failure") }
                override fun close() { closeEntered.countDown() }
            }
            val (started, _) = startNative(sessions, 1, engine, ticket)
            await(inNative)
            val closure = if (stopFirst) {
                close(sessions, id(1)).also { await(closeEntered); leave.countDown() }
            } else {
                leave.countDown()
                await(started)
                close(sessions, id(1))
            }
            await(started)
            await(closure.first)
            assertEquals(null, closure.second.get())
            assertEquals(AndroidNativeAdmission.State.Unknown, ledger.seal("speed-fence").captured.single().state)
            assertEquals("closed", sessions.status(id(1)))
        }
    }

    @Test fun queriedDnsCannotProduceExactReceiptEvenWhenLateCallbackReturns() {
        val ledger = ledger()
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger)
        val ticket = sessions.reserveOwner(id(1))
        val resolver = TransientResolverLifecycle()
        val lateCallback = CountDownLatch(1)
        val releaseCallback = CountDownLatch(1)
        val callbackDone = CountDownLatch(1)
        val queryDone = CountDownLatch(1)
        val oldContextWrites = AtomicInteger()
        val nextContextWrites = AtomicInteger()
        val engine = object : TransientSpeedtestSessions.Engine {
            private var proof = true
            override fun prepare() { }
            override fun start() {
                val query = resolver.enterQuery<Int>()
                Thread {
                    lateCallback.countDown()
                    await(releaseCallback)
                    query.publish(1)
                    callbackDone.countDown()
                }.start()
                Thread {
                    try { query.awaitAndDeliver { oldContextWrites.incrementAndGet() } }
                    catch (_: java.util.concurrent.CancellationException) { }
                    finally { query.returned(); queryDone.countDown() }
                }.start()
            }
            override fun close() {
                try { resolver.closeUnused() } catch (_: TransientResolverLifecycle.CleanupUnknown) { proof = false }
            }
            override fun cleanupConfirmed(): Boolean = proof
        }
        val (started, _) = startNative(sessions, 1, engine, ticket)
        await(started)
        await(lateCallback)
        val (closed, result) = close(sessions, id(1))
        await(closed)
        await(queryDone)
        assertEquals(null, result.get())
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
        assertEquals("closed", sessions.status(id(1)))
        val mainOwner = Any()
        var mainConnected = false
        sessions.withMainStart(mainOwner, { true }) { mainConnected = true }
        assertTrue(mainConnected)
        sessions.closeMain(mainOwner) { }
        val nextTicket = sessions.reserveOwner(id(2))
        val next = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { }
            override fun start() { }
            override fun close() { }
        }
        val (nextStarted, nextResult) = startNative(sessions, 2, next, nextTicket)
        await(nextStarted)
        assertEquals(null, nextResult.get())
        assertEquals("running", sessions.status(id(2)))
        val receipt = ledger.seal("speed-fence")
        assertEquals(4, receipt.coveredProducers.size)
        assertFalse(receipt.coverageComplete)
        releaseCallback.countDown()
        await(callbackDone)
        assertEquals(0, oldContextWrites.get())
        assertEquals(0, nextContextWrites.get())
        assertTrue(runCatching { resolver.enterQuery<Int>() }.isFailure)
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(nextTicket))
        assertTrue(TransientSpeedtestSessions.capabilities.isEmpty())
        val (nextClosed, _) = close(sessions, id(2))
        await(nextClosed)
    }

    @Test fun unusedResolverCanCloseAndRejectsQueriesThatArriveLater() {
        val resolver = TransientResolverLifecycle()
        resolver.closeUnused()
        assertTrue(runCatching { resolver.enterQuery<Int>() }.isFailure)
    }

    @Test fun actualNativeCloseFailureStillBlocksOrdinarySpeedtestAndMainConnection() {
        val ledger = ledger()
        val sessions = TransientSpeedtestSessions(nativeAdmission = ledger)
        val ticket = sessions.reserveOwner(id(1))
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { }
            override fun start() { }
            override fun close() { error("actual native/network Close failure") }
        }
        val (started, _) = startNative(sessions, 1, engine, ticket)
        await(started)
        val (closed, result) = close(sessions, id(1))
        await(closed)
        assertTrue(result.get()!!.contains("未知"))
        assertEquals("cleanupUnknown", sessions.status(id(1)))
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
        val untouched = AtomicInteger()
        assertTrue(runCatching { sessions.withMainStart(Any(), { true }) { untouched.incrementAndGet() } }.isFailure)
        val (next, nextResult) = startNative(sessions, 2, engine)
        await(next)
        assertTrue(nextResult.get()!!.contains("忙"))
        assertEquals(0, untouched.get())
    }
}
