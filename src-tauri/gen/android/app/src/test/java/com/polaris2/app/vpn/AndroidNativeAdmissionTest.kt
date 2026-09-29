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

    @Test fun idsUseOneNonBlankTrimmedUtf16Domain() {
        for (invalid in listOf("", " ", " process", "process ", "x".repeat(129))) {
            try { AndroidNativeAdmission(invalid); fail("invalid process nonce") }
            catch (_: IllegalArgumentException) {}
        }
        val ledger = open()
        for (invalid in listOf("", " ", " fence", "fence ", "😀".repeat(65))) {
            try { ledger.seal(invalid); fail("invalid fence ID") }
            catch (_: IllegalArgumentException) {}
        }
        for (invalid in listOf("", " ", " login", "login ", "a".repeat(257))) {
            try { ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, invalid); fail("invalid owner ID") }
            catch (_: IllegalArgumentException) {}
        }
        assertEquals("fence-ok", ledger.seal("fence-ok").fenceId)
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
        try { ledger.completeOperation(ticket); fail("validation claimed exact cleanup") }
        catch (_: IllegalArgumentException) {}
        assertTrue(ledger.validationCleanupUnknown(ticket))
        assertEquals(AndroidNativeAdmission.State.ValidationCleanupUnknown,
            ledger.receipt("fence-1").captured.single().state)
    }

    @Test fun validationReturningBeforeSealStillPoisonsDrain() {
        val ledger = open()
        val validation = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        assertTrue(ledger.enterBirth(validation))
        try { ledger.completeOperation(validation); fail("validation claimed Completed") }
        catch (_: IllegalArgumentException) {}
        assertTrue(ledger.validationCleanupUnknown(validation))
        assertEquals(AndroidNativeAdmission.State.ValidationCleanupUnknown,
            ledger.seal("fence-1").captured.single().state)
        val control = AndroidNativeAdmission().also { it.bootstrap(RequiredMarkerProof.Absent) }
        val targetless = control.reserveOperation(AndroidNativeAdmission.Kind.TargetlessReload)
        try { control.validationCleanupUnknown(targetless); fail("control mislabeled validation") }
        catch (_: IllegalArgumentException) {}
        assertTrue(control.completeOperation(targetless))
    }

    @Test fun terminalMethodsAreKindRestricted() {
        val ledger = open()
        val main = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "main-1")
        val validation = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        val control = ledger.reserveOperation(AndroidNativeAdmission.Kind.TargetlessStop)
        assertTrue(ledger.enterBirth(main))
        assertTrue(ledger.enterBirth(validation))
        assertTrue(ledger.enterBirth(control))
        try { ledger.completeOperation(main); fail("owner completed as operation") }
        catch (_: IllegalArgumentException) {}
        try { ledger.closedExact(validation); fail("validation closed as owner") }
        catch (_: IllegalArgumentException) {}
        try { ledger.validationCleanupUnknown(main); fail("owner became validation") }
        catch (_: IllegalArgumentException) {}
        assertTrue(ledger.closedExact(main))
        assertTrue(ledger.validationCleanupUnknown(validation))
        assertTrue(ledger.completeOperation(control))
        assertEquals(AndroidNativeAdmission.State.ValidationCleanupUnknown,
            ledger.seal("fence-1").captured.single().state)
    }
}
