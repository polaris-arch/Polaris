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
        val (started, startResult) = start(sessions, "blocked-start", engine)
        await(inStart)
        val (closed, closeResult) = close(sessions, "blocked-start")
        await(closed)
        await(started)
        assertEquals(null, closeResult.get())
        assertEquals("Android 临时测速已取消", startResult.get())
        assertEquals("closed", sessions.status("blocked-start"))
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
        val (started, _) = start(sessions, "before-main", engine)
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
        val (blocked, busy) = start(sessions, "during-main", engine)
        await(blocked)
        assertTrue(busy.get()!!.contains("忙"))
        releaseClose.countDown()
        await(mainDone)
        assertEquals("closed", sessions.status("before-main"))
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
        val (started, _) = start(sessions, "old", old)
        await(started)
        val (closed, closeResult) = close(sessions, "old")
        await(closeEntered)
        await(closed)
        assertTrue(closeResult.get()!!.contains("未知"))
        assertEquals("cleanupUnknown", sessions.status("old"))
        val mainEntered = AtomicInteger()
        val owner = Any()
        val rejected = runCatching { sessions.withMainStart(owner, { true }) { mainEntered.incrementAndGet() } }
        assertTrue(rejected.isFailure)
        assertEquals(0, mainEntered.get())
        val (blocked, _) = start(sessions, "new", old)
        await(blocked)
        assertEquals("unknown", sessions.status("new"))
        releaseClose.countDown()
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2)
        while (sessions.status("old") != "closed" && System.nanoTime() < deadline) Thread.yield()
        assertEquals("closed", sessions.status("old"))
        assertEquals(0, mainEntered.get())
        val newCloseCalls = AtomicInteger()
        val next = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() {}
            override fun close() { newCloseCalls.incrementAndGet() }
        }
        val (nextStarted, _) = start(sessions, "new", next)
        await(nextStarted)
        assertEquals("running", sessions.status("new"))
        assertEquals(1, oldCloseCalls.get())
        assertEquals(0, newCloseCalls.get())
        val (nextClosed, _) = close(sessions, "new")
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
        val (started, _) = start(sessions, "failed-close", engine)
        await(started)
        val (closed, result) = close(sessions, "failed-close")
        await(closed)
        assertTrue(result.get()!!.contains("未知"))
        assertEquals("cleanupUnknown", sessions.status("failed-close"))
        val (again, second) = close(sessions, "failed-close")
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
        val (started, _) = start(sessions, "factory", engine)
        await(preparing)
        val (closed, result) = close(sessions, "factory")
        await(closed)
        assertTrue(result.get()!!.contains("未知"))
        releasePrepare.countDown()
        await(started)
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2)
        while (sessions.status("factory") != "closed" && System.nanoTime() < deadline) Thread.yield()
        assertEquals("closed", sessions.status("factory"))
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
        val (started, result) = start(sessions, "failed-start", failed)
        await(started)
        assertTrue(result.get()!!.contains("启动失败"))
        val (closed, _) = close(sessions, "failed-start")
        await(closed)
        val good = object : TransientSpeedtestSessions.Engine {
            override fun prepare() {}
            override fun start() {}
            override fun close() {}
        }
        val (retried, retryResult) = start(sessions, "retry", good)
        await(retried)
        assertEquals(null, retryResult.get())
        val (wrong, wrongResult) = close(sessions, "failed-start-unknown")
        await(wrong)
        assertEquals(null, wrongResult.get())
        assertEquals("closed", sessions.status("failed-start-unknown"))
        assertEquals("running", sessions.status("retry"))
        val (end, _) = close(sessions, "retry")
        await(end)
    }

    @Test fun closeBeforeDelayedStartRevokesIdWithoutConstructingNativeEngine() {
        val sessions = TransientSpeedtestSessions()
        assertEquals("unknown", sessions.status("late"))
        val (closed, closeResult) = close(sessions, "late")
        await(closed)
        assertEquals(null, closeResult.get())
        assertEquals("closed", sessions.status("late"))
        val touched = AtomicInteger()
        val engine = object : TransientSpeedtestSessions.Engine {
            override fun prepare() { touched.incrementAndGet() }
            override fun start() { touched.incrementAndGet() }
            override fun close() { touched.incrementAndGet() }
        }
        val (started, result) = start(sessions, "late", engine)
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
        val (blocked, _) = start(sessions, "main-owned", engine)
        await(blocked)
        assertEquals(0, untouched.get())
        sessions.closeMain(owner) {}
        val (admitted, _) = start(sessions, "main-owned", engine)
        await(admitted)
        assertEquals("running", sessions.status("main-owned"))
        val (closed, _) = close(sessions, "main-owned")
        await(closed)
    }
}
