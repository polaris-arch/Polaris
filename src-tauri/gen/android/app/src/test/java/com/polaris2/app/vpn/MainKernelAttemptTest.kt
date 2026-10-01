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
    private val observedScope = TunScope(
        autoRoute = true,
        routes = listOf("0.0.0.0/0"),
        excludedRoutes = listOf("192.168.0.0/16"),
        skippedExcludes = emptyList(),
        allowedPackages = emptyList(),
        excludedPackages = listOf("com.example.bypass"),
        skippedPackages = emptyList(),
    )

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
        old.installTun(oldFd, observedScope)
        assertEquals(observedScope, old.currentTunScope())
        val detached = old.revokeAndDetachTun()
        assertEquals(null, old.currentTunScope())
        new.installTun(newFd, observedScope)
        val rejected = runCatching { old.installTun(lateFd, observedScope) }
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

    @Test fun lateOldTunCannotBePublishedToNewRegistryOwner() {
        val registry = MainKernelAttemptLedger()
        val old = MainKernelAttempt<Any>(runId = "old-run")
        val next = MainKernelAttempt<Any>(runId = "next-run")
        assertTrue(registry.claim(old))
        old.publish(Any())
        old.revokeAndDetachTun()
        old.closeOnce { }
        assertEquals(null, old.closed.get(2, TimeUnit.SECONDS))
        assertTrue(registry.completeAfterClose(old) { })
        assertTrue(registry.claim(next))
        val late = Closeable { }
        assertTrue(runCatching { registry.installTun(old, late, observedScope) }.isFailure)
        assertEquals(null, next.currentTunScope())
        val current = Closeable { }
        registry.installTun(next, current, observedScope)
        assertEquals(observedScope, next.currentTunScope())
    }

    @Test fun missingFdNeverProducesTunScope() {
        val attempt = MainKernelAttempt<Any>()
        assertEquals(null, attempt.currentTunScope())
        attempt.installTun(Closeable { }, observedScope)
        attempt.revokeAndDetachTun()
        assertEquals(null, attempt.currentTunScope())
    }

    @Test fun registryKeepsRunOwnedUntilConfirmedClose() {
        val registry = MainKernelAttemptLedger()
        val attempt = MainKernelAttempt<Any>(runId = "candidate-9")
        assertEquals("absent" to null, registry.snapshot())
        assertTrue(registry.claim(attempt))
        assertEquals("starting" to "candidate-9", registry.snapshot())
        attempt.acknowledgeStart()
        assertEquals("acknowledged" to "candidate-9", registry.snapshot())
        attempt.revokeAndDetachTun()
        assertEquals("closing" to "candidate-9", registry.snapshot())
        attempt.publish(Any())
        attempt.closeOnce { throw IllegalStateException("native close failed") }
        attempt.closed.get(2, TimeUnit.SECONDS)
        assertEquals("cleanupUnknown" to "candidate-9", registry.snapshot())
        assertFalse(registry.completeAfterClose(attempt) { })
        assertEquals("cleanupUnknown" to "candidate-9", registry.snapshot())
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

    @Test fun exactStopRejectsSameRunWithWrongBirthAndNeverCallsNewOwner() {
        val registry = MainKernelAttemptLedger()
        val old = MainKernelAttempt<Any>(runId = "reused-run")
        val next = MainKernelAttempt<Any>(runId = "reused-run")
        assertFalse(old.birthNonce == next.birthNonce)
        val oldCalls = AtomicInteger()
        val newCalls = AtomicInteger()
        assertTrue(registry.claim(old) { oldCalls.incrementAndGet() })
        val oldTarget = MainKernelExactTarget(old.runId, old.birthNonce)
        val wrong = MainKernelExactTarget(old.runId, next.birthNonce)
        val selectedBeforeRecreation = registry.exactCloseTarget(oldTarget).owner!!
        assertEquals("Unknown", registry.exactCloseTarget(wrong).result.state)
        assertEquals(null, registry.exactCloseTarget(wrong).owner)
        assertEquals(0, oldCalls.get())
        old.publish(Any())
        old.revokeAndDetachTun()
        old.closeOnce { }
        assertEquals(null, old.closed.get(2, TimeUnit.SECONDS))
        assertTrue(registry.completeAfterClose(old) { })
        assertTrue(registry.claim(next) { newCalls.incrementAndGet() })
        assertEquals("AlreadyGone", registry.exactStatus(oldTarget).state)
        assertEquals(null, registry.exactCloseTarget(oldTarget).owner)
        selectedBeforeRecreation.requestClose!!.invoke()
        assertEquals(1, oldCalls.get())
        assertEquals("Busy", registry.exactCloseTarget(wrong).result.state)
        assertEquals(0, newCalls.get())
    }

    @Test fun exactStopKeepsFactoryAndNativeCloseInCustodyUntilRelease() {
        val registry = MainKernelAttemptLedger()
        val attempt = MainKernelAttempt<Any>(runId = "candidate")
        val next = MainKernelAttempt<Any>(runId = "next")
        val target = MainKernelExactTarget(attempt.runId, attempt.birthNonce)
        val nativeEntered = CountDownLatch(1)
        val allowNativeClose = CountDownLatch(1)
        val closeRequests = AtomicInteger()
        val fdCloses = AtomicInteger()
        assertTrue(registry.claim(attempt) {
            closeRequests.incrementAndGet()
            val detached = attempt.revokeAndDetachTun()
            attempt.closeOnce {
                nativeEntered.countDown()
                await(allowNativeClose)
                detached?.close()
            }
        })
        attempt.installTun(Closeable { fdCloses.incrementAndGet() }, observedScope)
        val selected = registry.exactCloseTarget(target)
        assertEquals("Busy", selected.result.state)
        selected.owner!!.requestClose!!.invoke()
        assertEquals(1, closeRequests.get())
        assertEquals("Busy", registry.exactStatus(target).state)
        assertFalse(registry.claim(next))
        attempt.publish(Any()) // factory returned after Stop was selected
        await(nativeEntered)
        assertEquals("Busy", registry.exactStatus(target).state)
        assertFalse(registry.claim(next))
        assertEquals(0, fdCloses.get())
        allowNativeClose.countDown()
        assertEquals(null, attempt.closed.get(2, TimeUnit.SECONDS))
        assertTrue(registry.completeAfterClose(attempt) { })
        assertEquals("AlreadyGone", registry.exactStatus(target).state)
        assertEquals(1, fdCloses.get())
        assertTrue(registry.claim(next))
    }

    @Test fun absentAndFailedCloseNeverBecomeAlreadyGone() {
        val registry = MainKernelAttemptLedger()
        val attempt = MainKernelAttempt<Any>(runId = "candidate")
        val target = MainKernelExactTarget(attempt.runId, attempt.birthNonce)
        assertEquals("Unknown", registry.exactStatus(target).state)
        assertEquals("target-not-observed", registry.exactStatus(target).reason)
        assertTrue(registry.claim(attempt) { })
        attempt.publish(Any())
        attempt.revokeAndDetachTun()
        attempt.closeOnce { error("native close failed") }
        attempt.closed.get(2, TimeUnit.SECONDS)
        assertEquals("Unknown", registry.exactStatus(target).state)
        assertEquals("cleanup-unknown", registry.exactStatus(target).reason)
        assertEquals(null, registry.exactCloseTarget(target).owner)
        assertFalse(registry.claim(MainKernelAttempt<Any>()))
    }
}
