package com.polaris2.app.vpn

import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CancellationException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference

class TransientResolverLifecycleTest {
    private fun await(latch: CountDownLatch) = assertTrue(latch.await(2, TimeUnit.SECONDS))
    private class Queue : Executor {
        private val tasks = java.util.concurrent.LinkedBlockingQueue<Runnable>()
        override fun execute(command: Runnable) { tasks.add(command) }
        fun runNext() { checkNotNull(tasks.poll()).run() }
        val size get() = tasks.size
    }
    private fun cancelled(query: TransientResolverLifecycle.Query<Int>, writes: AtomicInteger = AtomicInteger()) {
        try { query.awaitAndDeliver { writes.incrementAndGet() }; fail("cancelled query delivered") }
        catch (_: CancellationException) { }
    }

    @Test fun callbackOnlyPublishesAndOriginalThreadDeliversOnce() {
        val lifecycle = TransientResolverLifecycle()
        val query = lifecycle.enterQuery<Int>()
        val writes = AtomicInteger()
        val callbackDone = CountDownLatch(1)
        Thread {
            assertTrue(query.publish(42))
            assertFalse(query.publish(99))
            assertEquals(0, writes.get())
            callbackDone.countDown()
        }.start()
        await(callbackDone)
        val original = Thread.currentThread()
        query.awaitAndDeliver { assertSame(original, Thread.currentThread()); assertEquals(42, it); writes.incrementAndGet() }
        assertTrue(runCatching { query.awaitAndDeliver { writes.incrementAndGet() } }.isFailure)
        query.returned()
        assertFalse(query.publish(100))
        assertEquals(1, writes.get())
        lifecycle.beginClose()
        assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(0, TimeUnit.SECONDS))
    }

    @Test fun pendingAndReadyResultsAreRevokedByCancelOrSeal() {
        for (publishFirst in listOf(false, true)) for (seal in listOf(false, true)) {
            val lifecycle = TransientResolverLifecycle()
            val query = lifecycle.enterQuery<Int>()
            val writes = AtomicInteger()
            if (publishFirst) assertTrue(query.publish(1))
            if (seal) lifecycle.beginClose() else query.cancel()
            assertFalse(query.publish(2))
            cancelled(query, writes)
            query.returned()
            assertEquals(0, writes.get())
            assertEquals(0, lifecycle.snapshot().calls)
        }
    }

    @Test fun closeCannotReturnPermitWhileJniIsBlocked() {
        val lifecycle = TransientResolverLifecycle()
        val query = lifecycle.enterQuery<Int>()
        val jniEntered = CountDownLatch(1)
        val jniRelease = CountDownLatch(1)
        val done = CountDownLatch(1)
        val writes = AtomicInteger()
        query.publish(1)
        Thread {
            try {
                query.awaitAndDeliver { jniEntered.countDown(); await(jniRelease); writes.incrementAndGet() }
            } finally { query.returned(); done.countDown() }
        }.start()
        await(jniEntered)
        lifecycle.beginClose()
        assertEquals(1, lifecycle.snapshot().calls)
        assertEquals(1, lifecycle.snapshot().deliveries)
        assertTrue(runCatching { query.returned() }.isFailure)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(1, TimeUnit.MILLISECONDS))
        assertEquals(1, lifecycle.snapshot().deliveries)
        jniRelease.countDown()
        await(done)
        assertEquals(1, writes.get())
        assertTrue(lifecycle.snapshot().locallyDrained)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(1, TimeUnit.SECONDS))
    }

    @Test fun jniThrowReturnsPermitOnlyInFinallyAndDoesNotAllowSecondDelivery() {
        val lifecycle = TransientResolverLifecycle()
        val query = lifecycle.enterQuery<Int>()
        val expected = IllegalStateException("JNI failed")
        query.publish(1)
        try { query.awaitAndDeliver { throw expected }; fail("expected failure") }
        catch (error: IllegalStateException) { assertSame(expected, error) }
        assertEquals(0, lifecycle.snapshot().deliveries)
        assertEquals(1, lifecycle.snapshot().calls)
        assertTrue(runCatching { query.awaitAndDeliver { fail("delivered twice") } }.isFailure)
        query.returned()
        assertEquals(0, lifecycle.snapshot().calls)
    }

    @Test fun cancelWakesOriginalBeforeQueuedSignalCancelRuns() {
        val cancellation = Queue()
        val lifecycle = TransientResolverLifecycle(cancellation)
        val query = lifecycle.enterQuery<Int>()
        val hook = TransientResolverCancelHook(query)
        val signalCalls = AtomicInteger()
        query.installCancellation { signalCalls.incrementAndGet() }
        val done = CountDownLatch(1)
        Thread { try { cancelled(query) } finally { hook.clear(); query.returned(); done.countDown() } }.start()
        hook.cancel()
        await(done)
        assertEquals(0, signalCalls.get())
        assertEquals(1, lifecycle.snapshot().cancellationTasks)
        lifecycle.beginClose()
        assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(1, TimeUnit.MILLISECONDS))
        assertEquals(1, lifecycle.snapshot().cancellationTasks)
        cancellation.runNext()
        hook.cancel()
        assertEquals(1, signalCalls.get())
        assertEquals(0, cancellation.size)
        assertEquals(0, lifecycle.snapshot().cancellationTasks)
    }

    @Test fun blockedSignalCancelStaysCountedWhileLocalCloseReturns() {
        val lifecycle = TransientResolverLifecycle()
        val query = lifecycle.enterQuery<Int>()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val signalDone = CountDownLatch(1)
        query.installCancellation { entered.countDown(); await(release); signalDone.countDown() }
        query.cancel()
        await(entered)
        cancelled(query)
        query.returned()
        lifecycle.beginClose()
        assertEquals(1, lifecycle.snapshot().cancellationTasks)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(1, TimeUnit.MILLISECONDS))
        assertEquals(1, lifecycle.snapshot().cancellationTasks)
        release.countDown()
        await(signalDone)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(1, TimeUnit.SECONDS))
        assertEquals(0, lifecycle.snapshot().cancellationTasks)
    }

    @Test fun cancellationThrowAndRejectedDispatchRemainUnknown() {
        val cancellation = Queue()
        val lifecycle = TransientResolverLifecycle(cancellation)
        val query = lifecycle.enterQuery<Int>()
        query.installCancellation { error("SDK cancel failure") }
        query.cancel()
        query.returned()
        cancellation.runNext()
        lifecycle.beginClose()
        assertTrue(lifecycle.snapshot().cancellationUnknown)
        assertEquals(0, lifecycle.snapshot().cancellationTasks)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(0, TimeUnit.SECONDS))

        val rejected = TransientResolverLifecycle(Executor { throw java.util.concurrent.RejectedExecutionException() })
        val rejectedQuery = rejected.enterQuery<Int>()
        rejectedQuery.installCancellation { fail("rejected task executed") }
        rejectedQuery.cancel()
        rejectedQuery.returned()
        rejected.beginClose()
        assertEquals(1, rejected.snapshot().cancellationTasks)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, rejected.awaitLocalDrain(1, TimeUnit.MILLISECONDS))
        assertEquals(1, rejected.snapshot().cancellationTasks)
    }

    @Test fun cancelBeforeSignalInstallationStillCancelsItExactlyOnce() {
        val cancellation = Queue()
        val lifecycle = TransientResolverLifecycle(cancellation)
        val query = lifecycle.enterQuery<Int>()
        query.cancel()
        query.installCancellation { }
        query.cancel()
        lifecycle.beginClose()
        assertEquals(1, cancellation.size)
        assertFalse(query.enterSdkSubmission())
        cancelled(query)
        query.returned()
        cancellation.runNext()
        assertEquals(0, lifecycle.snapshot().cancellationTasks)
        assertEquals(0L, lifecycle.snapshot().sdkSubmissions)
    }

    @Test fun partialSdkSubmissionThrowAndLateDuplicatesCannotWriteReturnedContext() {
        val sdk = Queue()
        val lifecycle = TransientResolverLifecycle()
        val query = lifecycle.enterQuery<Int>()
        val executor = lifecycle.sdkExecutor(sdk)
        val writes = AtomicInteger()
        val cleanup = AtomicInteger()
        try {
            assertTrue(query.enterSdkSubmission())
            executor.execute { query.publish(1); query.publish(2); cleanup.incrementAndGet() }
            error("SDK threw after scheduling a result/fd task")
        } catch (_: IllegalStateException) {
            query.cancel()
        } finally { query.returned() }
        lifecycle.beginClose()
        assertEquals(1L, lifecycle.snapshot().sdkSubmissions)
        assertEquals(1, lifecycle.snapshot().sdkTasks)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(1, TimeUnit.MILLISECONDS))
        assertEquals(1, lifecycle.snapshot().sdkTasks)
        sdk.runNext()
        executor.execute { assertFalse(query.publish(3)); cleanup.incrementAndGet() }
        sdk.runNext()
        assertEquals(2, cleanup.get())
        assertEquals(0, writes.get())
        assertEquals(0, lifecycle.snapshot().sdkTasks)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(1, TimeUnit.SECONDS))
    }

    @Test fun sdkTaskThrowStillCompletesItsRealRunnableWithoutDrainingUnknownSubmission() {
        val sdk = Queue()
        val lifecycle = TransientResolverLifecycle()
        val query = lifecycle.enterQuery<Int>()
        query.enterSdkSubmission()
        lifecycle.sdkExecutor(sdk).execute { error("SDK result failure") }
        query.cancel()
        query.returned()
        lifecycle.beginClose()
        assertTrue(runCatching { sdk.runNext() }.isFailure)
        assertEquals(0, lifecycle.snapshot().sdkTasks)
        assertEquals(1L, lifecycle.snapshot().sdkSubmissions)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(0, TimeUnit.SECONDS))
    }

    @Test fun apiBelow29BlockedLookupKeepsOriginalCallUntilActualReturn() {
        val lifecycle = TransientResolverLifecycle()
        val query = lifecycle.enterQuery<Int>()
        val lookupEntered = CountDownLatch(1)
        val lookupRelease = CountDownLatch(1)
        val done = CountDownLatch(1)
        val writes = AtomicInteger()
        Thread {
            try {
                assertTrue(query.canStartBlockingLookup())
                lookupEntered.countDown()
                await(lookupRelease) // Models synchronous Network.getAllByName, with no cancellation handle.
                assertFalse(query.publish(1))
                cancelled(query, writes)
            } finally { query.returned(); done.countDown() }
        }.start()
        await(lookupEntered)
        query.cancel()
        lifecycle.beginClose()
        assertEquals(1, lifecycle.snapshot().calls)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(1, TimeUnit.MILLISECONDS))
        assertEquals(1, lifecycle.snapshot().calls)
        assertEquals(1L, done.count)
        lookupRelease.countDown()
        await(done)
        assertEquals(0, writes.get())
        assertEquals(0, lifecycle.snapshot().calls)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(0, TimeUnit.SECONDS))
    }

    @Test fun interruptedOriginalRestoresInterruptAndRevokesLateCallback() {
        val lifecycle = TransientResolverLifecycle()
        val query = lifecycle.enterQuery<Int>()
        val entered = CountDownLatch(1)
        val done = CountDownLatch(1)
        val error = AtomicReference<Throwable?>()
        val thread = Thread {
            try {
                entered.countDown()
                query.awaitAndDeliver { fail("interrupted call delivered") }
                fail("expected interruption")
            } catch (failure: InterruptedException) {
                if (!Thread.currentThread().isInterrupted) error.set(AssertionError("interrupt not restored"))
            } catch (failure: Throwable) { error.set(failure) }
            finally { query.returned(); done.countDown() }
        }
        thread.start()
        await(entered)
        thread.interrupt()
        await(done)
        assertNull(error.get())
        assertFalse(query.publish(1))
        assertEquals(0, lifecycle.snapshot().calls)
    }

    @Test fun interruptWhileWaitingForPermitLockRevokesReadyWithoutEnteringJni() {
        val cancellation = Queue()
        val lifecycle = TransientResolverLifecycle(cancellation)
        val query = lifecycle.enterQuery<Int>()
        val gate = lifecycle.javaClass.getDeclaredField("gate").apply { isAccessible = true }.get(lifecycle)
        val writes = AtomicInteger()
        val signalCalls = AtomicInteger()
        val error = AtomicReference<Throwable?>()
        val interrupted = AtomicReference(false)
        val done = CountDownLatch(1)
        query.installCancellation { signalCalls.incrementAndGet() }
        assertTrue(query.publish(1))
        val worker = Thread {
            try { query.awaitAndDeliver { writes.incrementAndGet() } }
            catch (failure: Throwable) { error.set(failure) }
            finally {
                interrupted.set(Thread.currentThread().isInterrupted)
                query.returned()
                done.countDown()
            }
        }
        synchronized(gate) {
            worker.start()
            val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2)
            while (worker.state != Thread.State.BLOCKED && System.nanoTime() < deadline) Thread.yield()
            assertEquals(Thread.State.BLOCKED, worker.state)
            worker.interrupt()
            assertTrue(worker.isInterrupted)
        }
        await(done)
        assertEquals(0, writes.get())
        assertTrue(error.get() is InterruptedException)
        assertTrue(interrupted.get())
        assertEquals(0, lifecycle.snapshot().deliveries)
        assertEquals(0, lifecycle.snapshot().calls)
        assertFalse(query.publish(2))
        assertEquals(1, cancellation.size)
        assertEquals(1, lifecycle.snapshot().cancellationTasks)
        cancellation.runNext()
        assertEquals(1, signalCalls.get())
        assertEquals(0, lifecycle.snapshot().cancellationTasks)
    }

    @Test fun interruptionAfterPermitKeepsJniCountedUntilItsRealReturn() {
        val lifecycle = TransientResolverLifecycle()
        val query = lifecycle.enterQuery<Int>()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val interruptionObserved = CountDownLatch(1)
        val done = CountDownLatch(1)
        val writes = AtomicInteger()
        val error = AtomicReference<Throwable?>()
        val interrupted = AtomicReference(false)
        query.publish(1)
        val worker = Thread {
            try {
                query.awaitAndDeliver {
                    entered.countDown()
                    var sawInterrupt = false
                    while (true) {
                        try { release.await(); break }
                        catch (_: InterruptedException) {
                            // Models JNI that keeps running after its Java caller is interrupted.
                            sawInterrupt = true
                            interruptionObserved.countDown()
                        }
                    }
                    writes.incrementAndGet()
                    if (sawInterrupt) Thread.currentThread().interrupt()
                }
            } catch (failure: Throwable) { error.set(failure) }
            finally {
                interrupted.set(Thread.currentThread().isInterrupted)
                query.returned()
                done.countDown()
            }
        }
        worker.start()
        try {
            await(entered)
            worker.interrupt()
            await(interruptionObserved)
            lifecycle.beginClose()
            assertEquals(1, lifecycle.snapshot().calls)
            assertEquals(1, lifecycle.snapshot().deliveries)
            assertEquals(0, writes.get())
            assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(1, TimeUnit.MILLISECONDS))
            assertEquals(1, lifecycle.snapshot().deliveries)
            assertTrue(runCatching { query.returned() }.isFailure)
        } finally { release.countDown() }
        await(done)
        assertNull(error.get())
        assertTrue(interrupted.get())
        assertEquals(1, writes.get())
        assertEquals(0, lifecycle.snapshot().calls)
        assertEquals(0, lifecycle.snapshot().deliveries)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, lifecycle.awaitLocalDrain(0, TimeUnit.SECONDS))
    }

    @Test fun clearedLocalHookCannotCancelOrRetainAnActiveMailbox() {
        val cancellation = Queue()
        val lifecycle = TransientResolverLifecycle(cancellation)
        val query = lifecycle.enterQuery<Int>()
        val hook = TransientResolverCancelHook(query)
        query.installCancellation { fail("cleared hook cancelled") }
        hook.clear()
        hook.cancel()
        assertTrue(query.publish(1))
        query.awaitAndDeliver { }
        query.returned()
        hook.cancel()
        assertEquals(0, cancellation.size)
        assertEquals(0, lifecycle.snapshot().cancellationTasks)
    }

    @Test fun unusedCloseIsPositiveButEveryEnteredQueryRemainsUnknown() {
        val unused = TransientResolverLifecycle()
        unused.closeUnused()
        assertEquals(TransientResolverLifecycle.Drain.UnusedClosed, unused.awaitLocalDrain(0, TimeUnit.SECONDS))
        assertTrue(runCatching { unused.enterQuery<Int>() }.isFailure)
        val queried = TransientResolverLifecycle()
        val query = queried.enterQuery<Int>()
        query.cancel()
        query.returned()
        try { queried.closeUnused(); fail("queried close claimed cleanup") }
        catch (_: TransientResolverLifecycle.CleanupUnknown) { }
        assertEquals(TransientResolverLifecycle.Drain.Unknown, queried.awaitLocalDrain(0, TimeUnit.SECONDS))
    }
}
