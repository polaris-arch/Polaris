package com.polaris2.app.vpn

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.*
import org.junit.Test

class AndroidNativeControlOperationTest {
    private fun ledger(limit: Int = AndroidNativeAdmission.DEFAULT_MAX_METADATA_RECORDS) =
        AndroidNativeAdmission("control-process", maxMetadataRecords = limit).also {
            it.bootstrap(RequiredMarkerProof.Absent)
        }

    private fun owner(ledger: AndroidNativeAdmission, id: String = "owner-a"): MainKernelAttempt<String> {
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, id)
        assertTrue(ledger.enterBirth(ticket))
        return MainKernelAttempt<String>(runId = id, nativeTicket = ticket).also { it.publish(id) }
    }

    private fun reload(ledger: AndroidNativeAdmission) =
        AndroidNativeControlOperation.reserve(ledger, AndroidNativeAdmission.Kind.TargetlessReload)

    private fun await(latch: CountDownLatch) = assertTrue(latch.await(3, TimeUnit.SECONDS))

    @Test fun queuedSealRejectsFirstJniWithoutPoisoningExistingOwner() {
        val ledger = ledger()
        val attempt = owner(ledger)
        val operation = reload(ledger)
        val queued = CountDownLatch(1)
        val resume = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>()
        val calls = AtomicInteger()
        val worker = Thread {
            queued.countDown()
            await(resume)
            try { operation.runExact(attempt, { true }) { it.construct { calls.incrementAndGet() } } }
            catch (error: Throwable) { failure.set(error) }
        }.also { it.start() }
        await(queued)
        assertEquals(2, ledger.seal("fence").capturedCount)
        resume.countDown()
        worker.join(3000)
        assertFalse(worker.isAlive)
        assertTrue(failure.get() is AndroidNativeAdmission.AdmissionClosed)
        assertEquals(0, calls.get())
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(operation.ticket))
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(attempt.nativeTicket!!))
        assertFalse(attempt.constructionUnknown)
    }

    @Test fun capacityRejectsBothLastReservedControlAndAlreadyQueuedControl() {
        val ledger = ledger(4)
        val attempt = owner(ledger)
        val queued = reload(ledger)
        val last = reload(ledger)
        assertTrue(ledger.metadataUsage().capacityClosed)
        for (operation in listOf(queued, last)) {
            try {
                operation.runExact(attempt, { true }) { it.native { fail("capacity crossed JNI") } }
                fail("capacity accepted control")
            } catch (_: AndroidNativeAdmission.CapacityClosed) {}
            assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(operation.ticket))
        }
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(attempt.nativeTicket!!))
    }

    @Test fun unboundCallbacksCannotLookUpOrStopTheCurrentSuccessor() {
        val ledger = ledger()
        val successor = owner(ledger, "owner-b")
        for (kind in listOf(AndroidNativeAdmission.Kind.TargetlessStop, AndroidNativeAdmission.Kind.TargetlessReload)) {
            val ticket = AndroidNativeControlOperation.rejectUnbound(ledger, kind)
            assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(ticket))
        }
        assertFalse(successor.revoked)
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(successor.nativeTicket!!))
        ledger.seal("fence")
        try {
            AndroidNativeControlOperation.rejectUnbound(ledger, AndroidNativeAdmission.Kind.TargetlessStop)
            fail("sealed callback reserved")
        } catch (_: AndroidNativeAdmission.AdmissionClosed) {}
        assertFalse(successor.revoked)
    }

    @Test fun originalAttemptCheckDeclinesOldCallbackAndLeavesSuccessorUntouched() {
        val ledger = ledger()
        val original = owner(ledger)
        val successor = owner(ledger, "owner-b")
        val current = AtomicReference(successor)
        val operation = reload(ledger)
        assertFalse(operation.runExact(original, { current.get() === original }) {
            it.construct { fail("old callback operated on successor") }
        })
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(operation.ticket))
        assertFalse(successor.revoked)
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(successor.nativeTicket!!))
    }

    @Test fun successfulReloadCompletesOnlyItsOperationAndKeepsMainOwner() {
        val ledger = ledger()
        val attempt = owner(ledger)
        val operation = reload(ledger)
        val calls = AtomicInteger()
        assertTrue(operation.runExact(attempt, { true }) { boundary ->
            boundary.construct {
                assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(operation.ticket))
                calls.incrementAndGet()
            }
        })
        assertEquals(1, calls.get())
        assertEquals(AndroidNativeAdmission.State.Completed, ledger.state(operation.ticket))
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(attempt.nativeTicket!!))
        assertFalse(attempt.constructionUnknown)
    }

    @Test fun safeLoadErrorUsesSameAdmittedTicketWithoutChangingMainOwner() {
        val ledger = ledger()
        val attempt = owner(ledger)
        val operation = reload(ledger)
        val first = IllegalArgumentException("safe load failed")
        var visible: Throwable? = null
        operation.runExact(attempt, { true }) { boundary ->
            boundary.reportFailure(first) {
                assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(operation.ticket))
                visible = first
            }
        }
        assertSame(first, visible)
        assertEquals(0, first.suppressed.size)
        assertEquals(AndroidNativeAdmission.State.Completed, ledger.state(operation.ticket))
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(attempt.nativeTicket!!))
        assertFalse(attempt.constructionUnknown)
    }

    @Test fun preflightErrorCannotCrossSealToPublishErrorJni() {
        val ledger = ledger()
        val attempt = owner(ledger)
        val operation = reload(ledger)
        val first = IllegalArgumentException("config rejected")
        operation.runExact(attempt, { true }) { boundary ->
            ledger.seal("fence")
            boundary.reportFailure(first) { fail("setError bypassed seal") }
        }
        assertTrue(first.suppressed.single() is AndroidNativeAdmission.AdmissionClosed)
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(operation.ticket))
        assertFalse(attempt.constructionUnknown)
    }

    @Test fun constructorFailureIsUnknownBeforeErrorReportAndRetainsFirstError() {
        val ledger = ledger()
        val attempt = owner(ledger)
        val operation = reload(ledger)
        val first = IllegalStateException("partial construction")
        val reporting = IllegalArgumentException("setError failure")
        var observed: Throwable? = null
        try {
            operation.runExact(attempt, { true }) { boundary ->
                try { boundary.construct { throw first } }
                catch (error: Throwable) {
                    boundary.reportFailure(error) {
                        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(operation.ticket))
                        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(attempt.nativeTicket!!))
                        ledger.seal("fence")
                        throw reporting
                    }
                    throw error
                }
            }
        } catch (error: Throwable) { observed = error }
        assertSame(first, observed)
        assertSame(reporting, first.suppressed.single())
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(operation.ticket))
        attempt.closed.complete(null)
        attempt.released.complete(Unit)
        AndroidNativeMain.settleAfterExactRelease(ledger, attempt)
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(attempt.nativeTicket!!))
    }

    @Test fun errorReportingFailureAloneDoesNotInventMainConstructionUncertainty() {
        val ledger = ledger()
        val attempt = owner(ledger)
        val operation = reload(ledger)
        val first = IllegalArgumentException("safe load failed")
        val reporting = IllegalStateException("native reporting failed")
        operation.runExact(attempt, { true }) {
            it.reportFailure(first) { throw reporting }
        }
        assertSame(reporting, first.suppressed.single())
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(operation.ticket))
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(attempt.nativeTicket!!))
        assertFalse(attempt.constructionUnknown)
    }

    @Test fun enteredReloadStaysCapturedDuringStopTimeoutAndRevokedLateReturnStaysUnknown() {
        val ledger = ledger()
        val attempt = owner(ledger)
        val operation = reload(ledger)
        val entered = CountDownLatch(1)
        val resume = CountDownLatch(1)
        val closeStarted = CountDownLatch(1)
        val closedNative = AtomicInteger()
        val failure = AtomicReference<Throwable?>()
        val worker = Thread {
            try { operation.runExact(attempt, { true }) { boundary ->
                boundary.construct { entered.countDown(); await(resume) }
            } } catch (error: Throwable) { failure.set(error) }
        }.also { it.start() }
        await(entered)
        ledger.seal("fence")
        attempt.revokeAndDetachTun()
        attempt.closeOnce {
            closeStarted.countDown()
            synchronized(attempt.operationLock) { closedNative.incrementAndGet() }
        }
        await(closeStarted)
        try { attempt.closed.get(20, TimeUnit.MILLISECONDS); fail("close bypassed active JNI") }
        catch (_: TimeoutException) {}
        assertEquals(0, closedNative.get())
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(operation.ticket))
        assertEquals(2, ledger.receipt("fence").capturedCount)
        resume.countDown()
        worker.join(3000)
        assertFalse(worker.isAlive)
        assertTrue(failure.get() is AndroidNativeControlOperation.Revoked)
        assertNull(attempt.closed.get(3, TimeUnit.SECONDS))
        assertEquals(1, closedNative.get())
        attempt.released.complete(Unit)
        AndroidNativeMain.settleAfterExactRelease(ledger, attempt)
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(operation.ticket))
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(attempt.nativeTicket!!))
    }

    @Test fun stopBeforeFirstNativeBoundaryCancelsAndSuppressesOldErrorAndNotice() {
        val ledger = ledger()
        val attempt = owner(ledger)
        val operation = reload(ledger)
        val preflight = CountDownLatch(1)
        val resume = CountDownLatch(1)
        val first = IllegalArgumentException("late load failure")
        val failure = AtomicReference<Throwable?>()
        val worker = Thread {
            try { operation.runExact(attempt, { !attempt.revoked }) { boundary ->
                preflight.countDown()
                await(resume)
                boundary.reportFailure(first) { fail("late error crossed birth") }
                assertFalse(boundary.ifCurrent { fail("late notice reached successor") })
            } } catch (error: Throwable) { failure.set(error) }
        }.also { it.start() }
        await(preflight)
        attempt.revokeAndDetachTun()
        resume.countDown()
        worker.join(3000)
        assertFalse(worker.isAlive)
        assertNull(failure.get())
        assertTrue(first.suppressed.single() is AndroidNativeControlOperation.Revoked)
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(operation.ticket))
        assertFalse(attempt.constructionUnknown)
    }

    @Test fun existingOwnerCleanupStillSettlesAfterSealOrCapacityClosure() {
        for (capacity in listOf(false, true)) {
            val ledger = ledger(if (capacity) 3 else AndroidNativeAdmission.DEFAULT_MAX_METADATA_RECORDS)
            val attempt = owner(ledger)
            if (capacity) reload(ledger).cancelBeforeDispatch() else ledger.seal("fence")
            attempt.revokeAndDetachTun()
            attempt.closeOnce { synchronized(attempt.operationLock) { assertEquals("owner-a", it) } }
            assertNull(attempt.closed.get(3, TimeUnit.SECONDS))
            attempt.released.complete(Unit)
            AndroidNativeMain.settleAfterExactRelease(ledger, attempt)
            assertEquals(AndroidNativeAdmission.State.ClosedExact, ledger.state(attempt.nativeTicket!!))
        }
    }

    @Test fun failedQueueDispatchAndCallbackReplayCannotAcquireResources() {
        val ledger = ledger()
        val attempt = owner(ledger)
        val operation = reload(ledger)
        operation.cancelBeforeDispatch()
        operation.cancelBeforeDispatch()
        try {
            operation.runExact(attempt, { true }) { it.native { fail("cancelled dispatch reached JNI") } }
            fail("cancelled dispatch replayed")
        } catch (_: IllegalStateException) {}
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(operation.ticket))
    }

    @Test fun escapedBoundaryCannotWriteNativeAfterCallbackReturned() {
        val ledger = ledger()
        val attempt = owner(ledger)
        val operation = reload(ledger)
        lateinit var escaped: AndroidNativeControlOperation.Boundary
        operation.runExact(attempt, { true }) { escaped = it; it.native {} }
        try { escaped.native { fail("callback revived") }; fail("escaped boundary accepted") }
        catch (_: IllegalStateException) {}
        assertEquals(AndroidNativeAdmission.State.Completed, ledger.state(operation.ticket))
    }

    @Test fun lateRevocationCannotReplaceAnAlreadyObservedNativeError() {
        val ledger = ledger()
        val attempt = owner(ledger)
        val operation = reload(ledger)
        val first = IllegalStateException("first native failure")
        var observed: Throwable? = null
        try {
            operation.runExact(attempt, { true }) { boundary ->
                try { boundary.native { throw first } } catch (_: IllegalStateException) {}
                attempt.revokeAndDetachTun()
            }
        } catch (error: Throwable) { observed = error }
        assertSame(first, observed)
        assertTrue(first.suppressed.single() is AndroidNativeControlOperation.Revoked)
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(operation.ticket))
        assertFalse(attempt.constructionUnknown)
    }

    @Test fun failedExactTargetCheckCancelsReservationAndPreservesOriginalFailure() {
        val ledger = ledger()
        val attempt = owner(ledger)
        val operation = reload(ledger)
        val first = IllegalStateException("target check failed")
        var observed: Throwable? = null
        try {
            operation.runExact(attempt, { throw first }) { it.native { fail("target check bypassed") } }
        } catch (error: Throwable) { observed = error }
        assertSame(first, observed)
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(operation.ticket))
    }
}
