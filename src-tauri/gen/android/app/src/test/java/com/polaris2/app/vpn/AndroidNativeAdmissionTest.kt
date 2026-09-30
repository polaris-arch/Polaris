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
    private fun bounded(limit: Int) = AndroidNativeAdmission("capacity-process", maxMetadataRecords = limit).also {
        it.bootstrap(RequiredMarkerProof.Absent)
    }

    @Test fun metadataCountsOwnerOperationAndUnseenCloseAndClosesAtTheBoundary() {
        val ledger = bounded(4)
        val queued = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "main")
        val validation = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        assertEquals(AndroidNativeAdmission.MetadataUsage(2, 1, 4, false), ledger.metadataUsage())
        ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "unseen")
        assertEquals(AndroidNativeAdmission.MetadataUsage(2, 2, 4, true), ledger.metadataUsage())
        assertFalse(ledger.enterBirth(queued))
        assertFalse(ledger.enterBirth(validation))
        assertTrue(ledger.admissionRejection() is AndroidNativeAdmission.CapacityClosed)
        ledger.retireOwner(AndroidNativeAdmission.Kind.Main, "main")
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(queued))
        assertTrue(ledger.cancelBeforeBirth(validation))
        repeat(3) {
            try { ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig); fail("reopened") }
            catch (error: AndroidNativeAdmission.CapacityClosed) { assertTrue(error.message!!.contains("完全关闭并重新启动")) }
        }
        assertEquals(4, ledger.metadataUsage().records)
    }

    @Test fun lastReservedTicketAndAllEarlierQueuesCannotBirthWhenTheBudgetFills() {
        val ledger = bounded(3)
        val owner = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "queued")
        val last = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        assertTrue(ledger.metadataUsage().capacityClosed)
        assertFalse(ledger.enterBirth(owner))
        assertFalse(ledger.enterBirth(last))
        assertEquals(AndroidNativeAdmission.State.Reserved, ledger.state(last))
        try { ledger.receipt("capacity-is-not-a-fence"); fail("capacity created receipt") }
        catch (_: IllegalStateException) {}
        val receipt = ledger.seal("explicit-fence")
        assertEquals(setOf(owner.id, last.id), receipt.captured.map { it.ticket.id }.toSet())
        assertFalse(receipt.coverageComplete)
        assertEquals(3, receipt.coveredProducers.size)
    }

    @Test fun allocationThatCannotFitPermanentlyClosesEvenWithOneSlotLeft() {
        val ledger = bounded(2)
        val queued = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        try { ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "needs-two"); fail("over budget") }
        catch (_: AndroidNativeAdmission.CapacityClosed) {}
        assertEquals(1, ledger.metadataUsage().records)
        assertFalse(ledger.enterBirth(queued))
        try { ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "unseen-close"); fail("unrecorded tombstone allowed") }
        catch (_: AndroidNativeAdmission.CapacityClosed) {}
        assertEquals(1, ledger.metadataUsage().records)
    }

    @Test fun unseenCloseExhaustionClosesAllBirthAndDoesNotEvictTombstones() {
        val ledger = bounded(1)
        ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "first")
        repeat(3) { ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "first") }
        try { ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "second"); fail("unseen close accepted") }
        catch (_: AndroidNativeAdmission.CapacityClosed) {}
        try { ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "first"); fail("ABA") }
        catch (_: AndroidNativeAdmission.CapacityClosed) {}
        assertEquals(AndroidNativeAdmission.MetadataUsage(0, 1, 1, true), ledger.metadataUsage())
        assertTrue(ledger.seal("fence").captured.isEmpty())
    }

    @Test fun birthEnteredCanCloseButUnknownAndValidationRemainUnknownAtCapacity() {
        val ledger = bounded(7)
        val exact = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "exact")
        assertTrue(ledger.enterBirth(exact))
        val unknown = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "unknown")
        assertTrue(ledger.enterBirth(unknown))
        val validation = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        assertTrue(ledger.enterBirth(validation))
        assertTrue(ledger.unknown(unknown))
        val last = ledger.reserveOwner(AndroidNativeAdmission.Kind.Speedtest, "last")
        assertFalse(ledger.enterBirth(last))
        ledger.retireOwner(AndroidNativeAdmission.Kind.Main, "exact")
        assertTrue(ledger.closedExact(exact))
        assertFalse(ledger.closedExact(unknown))
        assertTrue(ledger.validationCleanupUnknown(validation))
        val receipt = ledger.seal("fence")
        assertEquals(setOf(unknown.id, validation.id, last.id), receipt.captured.map { it.ticket.id }.toSet())
    }

    @Test fun invalidIdsAndDuplicateOwnersDoNotConsumeOrCloseCapacity() {
        val ledger = bounded(3)
        ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "used")
        for (id in listOf("used", " ", "a".repeat(257))) {
            try { ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, id); fail("invalid ID admitted") }
            catch (_: IllegalStateException) {} catch (_: IllegalArgumentException) {}
        }
        assertEquals(AndroidNativeAdmission.MetadataUsage(0, 1, 3, false), ledger.metadataUsage())
    }

    @Test fun concurrentReserveAndRetireCannotOverrunTheCombinedBudget() {
        repeat(20) { turn ->
            val ledger = bounded(31)
            val release = CountDownLatch(1)
            val errors = AtomicReference<Throwable?>()
            val workers = (0 until 40).map { index -> Thread {
                try {
                    check(release.await(2, TimeUnit.SECONDS))
                    if (index % 2 == 0) ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "close-$turn-$index")
                    else ledger.reserveOwner(AndroidNativeAdmission.Kind.Speedtest, "start-$turn-$index")
                } catch (_: AndroidNativeAdmission.CapacityClosed) {} catch (error: Throwable) { errors.set(error) }
            }.also { it.start() } }
            release.countDown()
            workers.forEach { it.join(2_000); assertFalse(it.isAlive) }
            assertEquals(null, errors.get())
            val usage = ledger.metadataUsage()
            assertTrue(usage.records <= 31)
            assertTrue(usage.capacityClosed)
            assertTrue(ledger.seal("fence").captured.all { !ledger.enterBirth(it.ticket) })
        }
    }
    private fun open() = AndroidNativeAdmission("process-1").also {
        it.bootstrap(RequiredMarkerProof.Absent)
    }

    @Test fun partialProductionManifestNeverClaimsCompleteCoverage() {
        val receipt = open().seal("fence-1")
        assertEquals(1, receipt.protocolVersion)
        assertEquals(setOf("main.bridge", "main.system", "validation.checkConfig"),
            receipt.coveredProducers.toSet())
        assertFalse(receipt.coverageComplete)
        assertEquals(0, receipt.capturedCount)
        assertTrue(receipt.captured.isEmpty())
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

    @Test fun idsUseOneExplicitAsciiDomain() {
        val invalidCharacters = listOf("\u001c", "\u0085", "\uD800", "😀")
        for (invalid in listOf("", " ", " process", "process ", "x".repeat(129)) + invalidCharacters) {
            try { AndroidNativeAdmission(invalid); fail("invalid process nonce") }
            catch (_: IllegalArgumentException) {}
        }
        val ledger = open()
        for (invalid in listOf("", " ", " fence", "fence ", "😀".repeat(65)) + invalidCharacters) {
            try { ledger.seal(invalid); fail("invalid fence ID") }
            catch (_: IllegalArgumentException) {}
        }
        for (invalid in listOf("", " ", " login", "login ", "a".repeat(257)) + invalidCharacters) {
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
