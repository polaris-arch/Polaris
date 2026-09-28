package com.polaris2.app.vpn

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TransientSpeedtestSessionsTest {
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
        sessions.withMainStart(owner, { true }) {}
        assertTrue(runCatching { sessions.closeMain(owner) { error("native main close failed") } }.isFailure)
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
}
