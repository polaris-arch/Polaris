package com.polaris2.app.vpn

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class AndroidNativeAdmissionTest {
    private fun open() = AndroidNativeAdmission("process-1").also {
        it.bootstrap(RequiredMarkerProof.Absent)
    }

    @Test fun coldMarkerMustBeProvedAbsentAndCannotBeReopened() {
        for (proof in listOf(RequiredMarkerProof.PresentOrUnknown)) {
            val ledger = AndroidNativeAdmission("process-blocked")
            ledger.bootstrap(proof)
            try { ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "run-1"); fail("opened") }
            catch (_: AndroidNativeAdmission.AdmissionClosed) {}
            ledger.seal("fence-1")
            try { ledger.bootstrap(RequiredMarkerProof.Absent); fail("rebootstrapped") }
            catch (_: IllegalStateException) {}
        }
    }

    @Test fun sealCapturesQueuedWorkAndBlocksLaterNativeBirth() {
        val ledger = open()
        val queued = ledger.reserveOwner(AndroidNativeAdmission.Kind.Speedtest, "speed-1")
        val operation = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        val sealed = ledger.seal("fence-1")
        assertEquals(setOf(queued.id, operation.id), sealed.captured.map { it.ticket.id }.toSet())
        assertFalse(ledger.enterBirth(queued))
        assertTrue(ledger.cancelBeforeBirth(queued))
        assertTrue(ledger.cancelBeforeBirth(operation))
        assertEquals(sealed.sealedRevision, ledger.seal("fence-1").sealedRevision)
        try { ledger.seal("fence-2"); fail("second fence") }
        catch (_: IllegalStateException) {}
        try { ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "run-2"); fail("reopened") }
        catch (_: AndroidNativeAdmission.AdmissionClosed) {}
    }

    @Test fun consumedOwnerIdentityAndUnknownCannotBeErased() {
        val ledger = open()
        val owner = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "login-1")
        assertTrue(ledger.enterBirth(owner))
        assertTrue(ledger.unknown(owner))
        val receipt = ledger.seal("fence-1")
        assertEquals(AndroidNativeAdmission.State.Unknown, receipt.captured.single().state)
        assertFalse(ledger.closedExact(owner))
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.receipt("fence-1").captured.single().state)
    }

    @Test fun closeBeforeStartAndCloseAfterReservationCannotReplayLoginId() {
        val ledger = open()
        ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "early-close")
        try { ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "early-close"); fail("replayed") }
        catch (_: IllegalStateException) {}
        val queued = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "queued-close")
        ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "queued-close")
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(queued))
        assertFalse(ledger.enterBirth(queued))
        try { ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "queued-close"); fail("replayed") }
        catch (_: IllegalStateException) {}
    }

    @Test fun sealVersusQueuedBirthHasOneWinner() {
        repeat(100) { turn ->
            val ledger = open()
            val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "run-$turn")
            val ready = CountDownLatch(1)
            val release = CountDownLatch(1)
            val entered = AtomicReference<Boolean>()
            val worker = Thread {
                ready.countDown()
                check(release.await(2, TimeUnit.SECONDS))
                entered.set(ledger.enterBirth(ticket))
            }
            worker.start()
            assertTrue(ready.await(2, TimeUnit.SECONDS))
            val sealed = ledger.seal("fence-$turn")
            release.countDown()
            worker.join(2_000)
            assertFalse(worker.isAlive)
            assertEquals(false, entered.get())
            assertEquals(ticket.id, sealed.captured.single().ticket.id)
            assertTrue(ledger.cancelBeforeBirth(ticket))
        }
    }

    @Test fun inFlightValidationRetainsCleanupUnknownAfterReturn() {
        val ledger = open()
        val ticket = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        assertTrue(ledger.enterBirth(ticket))
        ledger.seal("fence-1")
        assertTrue(ledger.validationCleanupUnknown(ticket))
        assertEquals(AndroidNativeAdmission.State.ValidationCleanupUnknown,
            ledger.receipt("fence-1").captured.single().state)
    }
}
