package com.polaris2.app.vpn

import java.io.Closeable
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class MainKernelAttemptTest {
    private fun await(latch: CountDownLatch) = assertTrue(latch.await(2, TimeUnit.SECONDS))

    @Test fun stopBeforeFactoryPublishesWaitsForThatServerAndClosesExactlyOnce() {
        val attempt = MainKernelAttempt<Any>()
        val server = Any()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val calls = AtomicInteger()
        attempt.revokeAndDetachTun()
        attempt.closeOnce {
            assertSame(server, it)
            calls.incrementAndGet()
            entered.countDown()
            await(release)
        }
        attempt.closeOnce { error("second close job ran") }
        assertFalse(entered.await(30, TimeUnit.MILLISECONDS))
        assertFalse(attempt.closed.isDone)
        attempt.publish(server)
        await(entered)
        assertFalse(attempt.closed.isDone)
        release.countDown()
        assertEquals(null, attempt.closed.get(2, TimeUnit.SECONDS))
        assertEquals(1, calls.get())
    }

    @Test fun failedCloseIsStickyAndCannotBeAcknowledgedByAnotherStop() {
        val attempt = MainKernelAttempt<Any>()
        attempt.publish(Any())
        attempt.closeOnce { throw IllegalStateException("native close failed") }
        assertEquals("native close failed", attempt.closed.get(2, TimeUnit.SECONDS)?.message)
        attempt.closeOnce { error("retry must not overwrite the original result") }
        assertEquals("native close failed", attempt.closed.getNow(null)?.message)
    }

    @Test fun revokedGenerationRejectsLateTunAndDetachesOnlyItsOwnDescriptor() {
        val old = MainKernelAttempt<Any>()
        val new = MainKernelAttempt<Any>()
        val oldCloses = AtomicInteger()
        val newCloses = AtomicInteger()
        val lateCloses = AtomicInteger()
        val oldFd = Closeable { oldCloses.incrementAndGet() }
        val newFd = Closeable { newCloses.incrementAndGet() }
        val lateFd = Closeable { lateCloses.incrementAndGet() }
        old.installTun(oldFd)
        val detached = old.revokeAndDetachTun()
        new.installTun(newFd)
        val rejected = runCatching { old.installTun(lateFd) }
        if (rejected.isFailure) lateFd.close() // BoxService closes rejected establish results.
        assertTrue(rejected.isFailure)
        assertSame(oldFd, detached)
        detached?.close()
        assertEquals(1, oldCloses.get())
        assertEquals(1, lateCloses.get())
        assertEquals(0, newCloses.get())
        new.revokeAndDetachTun()?.close()
        assertEquals(1, newCloses.get())
    }

    @Test fun secondServiceCannotClaimWhileFirstCloseIsUnknownOrFailed() {
        val registry = MainKernelAttemptLedger()
        val first = MainKernelAttempt<Any>()
        val second = MainKernelAttempt<Any>()
        assertTrue(registry.claim(first))
        assertFalse(registry.claim(second))
        first.publish(Any())
        first.closeOnce { throw IllegalStateException("native close failed") }
        assertEquals("native close failed", first.closed.get(2, TimeUnit.SECONDS)?.message)
        assertFalse(registry.completeAfterClose(first) { error("false bridge ACK") })
        assertFalse(registry.abandon(first))
        assertFalse(registry.claim(second))
        assertTrue(registry.isCurrent(first))
        // A process with failed native cleanup deliberately keeps this global owner.
        // A fresh process is required; no later nil-server Stop may turn it into success.
    }

    @Test fun confirmedCloseAtomicallySettlesOldBridgeBeforeNewServiceCanClaim() {
        val registry = MainKernelAttemptLedger()
        val first = MainKernelAttempt<Any>()
        val second = MainKernelAttempt<Any>()
        assertTrue(registry.claim(first))
        first.publish(Any())
        first.closeOnce { }
        assertEquals(null, first.closed.get(2, TimeUnit.SECONDS))
        val ack = CountDownLatch(1)
        val release = CountDownLatch(1)
        val done = CountDownLatch(1)
        Thread {
            assertTrue(registry.completeAfterClose(first) {
                ack.countDown()
                await(release)
            })
            done.countDown()
        }.start()
        await(ack)
        val claimed = CountDownLatch(1)
        val result = java.util.concurrent.atomic.AtomicReference<Boolean>()
        Thread { result.set(registry.claim(second)); claimed.countDown() }.start()
        assertFalse(claimed.await(30, TimeUnit.MILLISECONDS))
        release.countDown()
        await(done)
        await(claimed)
        assertEquals(true, result.get())
        assertFalse(registry.completeAfterClose(first) { error("old callback reached new bridge") })
        assertTrue(registry.isCurrent(second))
    }
}
