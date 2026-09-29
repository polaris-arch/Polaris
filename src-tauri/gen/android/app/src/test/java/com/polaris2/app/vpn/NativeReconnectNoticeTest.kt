package com.polaris2.app.vpn

import java.io.File
import java.nio.file.Files
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NativeReconnectNoticeTest {
    private inline fun withMarker(test: (File) -> Unit) {
        val dir = Files.createTempDirectory("polaris-native-reconnect-test").toFile()
        try { test(File(dir, "native-reconnect-required")) }
        finally { dir.deleteRecursively() }
    }

    @Test fun automaticRejectedBirthDoesNotRequestNoticeClear() = withMarker { marker ->
        val rejected = MainKernelAttempt<Any>()
        NativeReconnectNotice.require(marker, rejected.birthNonce)
        assertFalse(rejected.clearReconnectNoticeOnClose)
        assertEquals(rejected.birthNonce, NativeReconnectNotice.owner(marker))
    }

    @Test fun oldCloseCannotEraseLaterAttemptNotice() = withMarker { marker ->
        val old = MainKernelAttempt<Any>()
        val successor = MainKernelAttempt<Any>()
        NativeReconnectNotice.require(marker, old.birthNonce)
        NativeReconnectNotice.require(marker, successor.birthNonce)
        assertFalse(NativeReconnectNotice.clearIfOwner(marker, old.birthNonce))
        assertEquals(successor.birthNonce, NativeReconnectNotice.owner(marker))
        assertTrue(NativeReconnectNotice.clearIfOwner(marker, successor.birthNonce))
        assertNull(NativeReconnectNotice.owner(marker))
    }

    @Test fun explicitStopOrFreshBridgeMayClearTheExactReason() = withMarker { marker ->
        val attempt = MainKernelAttempt<Any>()
        NativeReconnectNotice.require(marker, attempt.birthNonce)
        attempt.clearReconnectNoticeOnClose = true
        assertTrue(attempt.clearReconnectNoticeOnClose)
        assertTrue(NativeReconnectNotice.clearIfOwner(marker, attempt.birthNonce))
        NativeReconnectNotice.require(marker, attempt.birthNonce)
        NativeReconnectNotice.clear(marker)
        assertNull(NativeReconnectNotice.owner(marker))
    }

    @Test fun delayedOldPublishAfterClearOrSuccessorCannotRaiseGhostNotification() = withMarker { marker ->
        val old = MainKernelAttempt<Any>()
        NativeReconnectNotice.require(marker, old.birthNonce)
        val paused = CountDownLatch(1)
        val resume = CountDownLatch(1)
        val sent = AtomicInteger()
        val published = AtomicBoolean(true)
        val publisher = Thread {
            paused.countDown()
            check(resume.await(2, TimeUnit.SECONDS))
            published.set(NativeReconnectNotice.publishIfOwner(marker, old.birthNonce) {
                sent.incrementAndGet()
            })
        }
        publisher.start()
        assertTrue(paused.await(2, TimeUnit.SECONDS))
        assertTrue(NativeReconnectNotice.clearIfOwner(marker, old.birthNonce))
        resume.countDown()
        publisher.join(2_000)
        assertFalse(publisher.isAlive)
        assertFalse(published.get())
        assertEquals(0, sent.get())

        val successor = MainKernelAttempt<Any>()
        NativeReconnectNotice.require(marker, successor.birthNonce)
        assertFalse(NativeReconnectNotice.publishIfOwner(marker, old.birthNonce) { sent.incrementAndGet() })
        assertEquals(0, sent.get())
        assertEquals(successor.birthNonce, NativeReconnectNotice.owner(marker))
    }

    @Test fun publishAndClearAreSerializedInEitherOrder() = withMarker { marker ->
        val attempt = MainKernelAttempt<Any>()
        NativeReconnectNotice.require(marker, attempt.birthNonce)
        val publishing = CountDownLatch(1)
        val finishPublish = CountDownLatch(1)
        val clearStarted = CountDownLatch(1)
        val clearDone = CountDownLatch(1)
        val publisher = Thread {
            NativeReconnectNotice.publishIfOwner(marker, attempt.birthNonce) {
                publishing.countDown()
                check(finishPublish.await(2, TimeUnit.SECONDS))
            }
        }
        publisher.start()
        assertTrue(publishing.await(2, TimeUnit.SECONDS))
        val clearer = Thread {
            clearStarted.countDown()
            NativeReconnectNotice.clearIfOwner(marker, attempt.birthNonce)
            clearDone.countDown()
        }
        clearer.start()
        assertTrue(clearStarted.await(2, TimeUnit.SECONDS))
        assertFalse(clearDone.await(50, TimeUnit.MILLISECONDS))
        finishPublish.countDown()
        publisher.join(2_000)
        clearer.join(2_000)
        assertFalse(publisher.isAlive)
        assertFalse(clearer.isAlive)
        assertNull(NativeReconnectNotice.owner(marker))
    }

    @Test fun alreadyStoppedIntentMarksTheCurrentNativeAttemptOrClearsVacantMarker() = withMarker { marker ->
        val registry = MainKernelAttemptLedger()
        val closing = MainKernelAttempt<Any>()
        assertTrue(registry.claim(closing))
        NativeReconnectNotice.require(marker, closing.birthNonce)
        val resumeClose = CountDownLatch(1)
        val closed = Thread {
            check(resumeClose.await(2, TimeUnit.SECONDS))
            closing.closed.complete(null)
            check(registry.completeAfterClose(closing) {})
            if (closing.clearReconnectNoticeOnClose) {
                NativeReconnectNotice.clearIfOwner(marker, closing.birthNonce)
            }
        }
        closed.start()
        assertTrue(registry.requestReconnectNoticeDismissal(
            { NativeReconnectNotice.owner(marker) },
            { NativeReconnectNotice.clearIfOwner(marker, it) },
        ))
        assertTrue(closing.clearReconnectNoticeOnClose)
        resumeClose.countDown()
        closed.join(2_000)
        assertFalse(closed.isAlive)
        assertNull(NativeReconnectNotice.owner(marker))

        NativeReconnectNotice.require(marker, closing.birthNonce)
        assertTrue(registry.requestReconnectNoticeDismissal(
            { NativeReconnectNotice.owner(marker) },
            { NativeReconnectNotice.clearIfOwner(marker, it) },
        ))
        assertNull(NativeReconnectNotice.owner(marker))

        val successor = MainKernelAttempt<Any>()
        assertTrue(registry.claim(successor))
        NativeReconnectNotice.require(marker, closing.birthNonce)
        assertTrue(registry.requestReconnectNoticeDismissal(
            { NativeReconnectNotice.owner(marker) },
            { NativeReconnectNotice.clearIfOwner(marker, it) },
        ))
        assertTrue(successor.clearReconnectNoticeOnClose)
        assertFalse(NativeReconnectNotice.clearIfOwner(marker, successor.birthNonce))
        assertEquals(closing.birthNonce, NativeReconnectNotice.owner(marker))
    }

    @Test fun stopBeforeRejectedSystemNoticeWriteStillClearsOnExactClose() = withMarker { marker ->
        val registry = MainKernelAttemptLedger()
        val rejected = MainKernelAttempt<Any>()
        assertTrue(registry.claim(rejected))
        val resumeClose = CountDownLatch(1)
        val closed = Thread {
            check(resumeClose.await(2, TimeUnit.SECONDS))
            rejected.closed.complete(null)
            check(registry.completeAfterClose(rejected) {})
            if (rejected.clearReconnectNoticeOnClose) {
                NativeReconnectNotice.clearIfOwner(marker, rejected.birthNonce)
            }
        }
        closed.start()
        assertNull(NativeReconnectNotice.owner(marker))
        assertTrue(registry.requestReconnectNoticeDismissal(
            { error("the current native owner must be marked before reading a not-yet-written marker") },
            { NativeReconnectNotice.clearIfOwner(marker, it) },
        ))
        assertTrue(rejected.clearReconnectNoticeOnClose)
        NativeReconnectNotice.require(marker, rejected.birthNonce)
        resumeClose.countDown()
        closed.join(2_000)
        assertFalse(closed.isAlive)
        assertNull(NativeReconnectNotice.owner(marker))
        val notifications = AtomicInteger()
        assertFalse(NativeReconnectNotice.publishIfOwner(marker, rejected.birthNonce) {
            notifications.incrementAndGet()
        })
        assertEquals(0, notifications.get())
    }
}
