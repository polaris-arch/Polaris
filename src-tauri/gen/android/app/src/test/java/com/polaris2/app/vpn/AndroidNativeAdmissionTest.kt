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

    @Test fun originalStoreCapturesWholeReloadHistoryAndNeverPromotesGlobalUnknown() {
        val ledger = open()
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "native-main")
        val custody = ledger.bindTailscaleStore(ticket, "original-birth")
        assertTrue(ledger.enterBirth(ticket))
        var native = ScopedNativeFixture.export()
        val first = ScopedNativeFixture.run("a", "first", listOf("one"))
        custody.invoke("first", { native }) { native = ScopedNativeFixture.export(first) }
        val boundBeforeReload = custody.wire()
        val second = ScopedNativeFixture.run("b", "reload", listOf("two"))
        custody.invoke("reload", { native }) { native = ScopedNativeFixture.export(first, second) }
        assertFalse(custody.matches(boundBeforeReload))
        assertEquals(2, custody.wire().getJSONArray("originalObservedRuns").length())
        assertEquals(ScopedNativeFixture.digest("reload"), custody.wire().getString("actualConfigDigest"))
        assertEquals(null, custody.closedRelation(ScopedNativeFixture.file("one")))
        native = ScopedNativeFixture.export(ScopedNativeFixture.retired(first), ScopedNativeFixture.retired(second))
        custody.closed { native }
        assertEquals(true, custody.closedRelation(ScopedNativeFixture.file("one")))
        assertEquals(true, custody.closedRelation(ScopedNativeFixture.file("two")))
        assertEquals(false, custody.closedRelation(ScopedNativeFixture.file("other")))
        assertTrue(ledger.unknown(ticket))
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
        assertFalse(ledger.closedExact(ticket))
        val query = ledger.tailscaleTargetAction(custody.wire(), ScopedNativeFixture.file("one"), "query", "query")
        assertEquals("Unknown", query.getJSONArray("entries").getJSONObject(0).getString("globalState"))
    }

    @Test fun originalLoginReadonlyRetirementSurvivesSuccessorWithoutClosingOrRebinding() {
        val ledger = open()
        val oldTicket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "old-login")
        val old = ledger.bindTailscaleStore(oldTicket)
        assertTrue(ledger.enterBirth(oldTicket))
        var native = ScopedNativeFixture.export()
        val run = ScopedNativeFixture.run("a", "old-config", listOf("one"))
        old.invoke("old-config", { native }) { native = ScopedNativeFixture.export(run) }
        val binding = old.wire()
        native = ScopedNativeFixture.export(ScopedNativeFixture.retired(run))
        old.closed { native }
        assertTrue(ledger.unknown(oldTicket))
        val nextTicket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "next-login")
        val next = ledger.bindTailscaleStore(nextTicket)
        assertTrue(ledger.enterBirth(nextTicket))
        val before = ledger.seal("readonly-fence")
        val retired = checkNotNull(ledger.readTailscaleOwner(binding))
        assertEquals("old-login", retired.getString("logicalInstanceId"))
        assertTrue(retired.getBoolean("terminal"))
        assertEquals(before, ledger.receipt("readonly-fence"))
        assertFalse(next.wire().getBoolean("terminal"))
        assertEquals(null, ledger.readTailscaleOwner(org.json.JSONObject(binding.toString()).put("nativeTicketId", nextTicket.id)))
        assertEquals(null, ledger.readTailscaleOwner(org.json.JSONObject(binding.toString()).put("logicalInstanceId", "next-login")))
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(oldTicket))
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(nextTicket))
    }

    @Test fun missingReboundLateOrThrowingNativeHistoryCannotBeRepairedByOrdinaryClose() {
        for (failure in listOf("missing-getter", "throw", "rebound", "missing-run", "extra-run")) {
            val ledger = open()
            val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "login-$failure")
            assertTrue(ledger.enterBirth(ticket))
            val custody = ledger.bindTailscaleStore(ticket)
            var native = ScopedNativeFixture.export()
            val first = ScopedNativeFixture.run("a", "first", listOf("one"))
            custody.invoke("first", { native }) { native = ScopedNativeFixture.export(first) }
            val second = ScopedNativeFixture.run("b", "second", listOf("one"))
            val result = runCatching {
                custody.invoke("second", { if (failure == "missing-getter") error("old AAR") else native }) {
                    native = when (failure) {
                        "rebound" -> ScopedNativeFixture.export(ScopedNativeFixture.run("a", "changed", listOf("one")), second)
                        "missing-run" -> ScopedNativeFixture.export(second)
                        "extra-run" -> ScopedNativeFixture.export(first, second, ScopedNativeFixture.run("c", "second", emptyList()))
                        else -> ScopedNativeFixture.export(first, second)
                    }
                    if (failure == "throw") error("original JNI failure")
                }
            }
            assertEquals(failure == "throw", result.isFailure)
            custody.closed { ScopedNativeFixture.export(ScopedNativeFixture.retired(first), ScopedNativeFixture.retired(second)) }
            assertTrue(custody.wire().getBoolean("historyUnknown"))
            assertEquals(null, custody.closedRelation(ScopedNativeFixture.file("one")))
            assertTrue(ledger.unknown(ticket))
            assertTrue(runCatching { ledger.tailscaleTargetAction(custody.wire(), ScopedNativeFixture.file("one"), "query", "query") }.isFailure)
        }
    }

    @Test fun targetActionReservationBlocksBirthUntilTheExactOriginalFinish() {
        val ledger = open()
        val (ticket, custody) = ScopedNativeFixture.closedOwner(ledger, "original")
        val binding = custody.wire()
        val file = ScopedNativeFixture.file("one")
        val begun = ledger.tailscaleTargetAction(binding, file, "action-one", "begin")
        assertEquals("action-one", begun.getString("actionRequestId"))
        val pending = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "successor")
        assertFalse(ledger.enterBirth(pending))
        assertTrue(runCatching { ledger.tailscaleTargetAction(binding, file, "wrong-action", "finish") }.isFailure)
        assertFalse(ledger.enterBirth(pending))
        val foreign = org.json.JSONObject(binding.toString()).put("nativeTicketId", pending.id)
        assertTrue(runCatching { ledger.tailscaleTargetAction(foreign, file, "action-one", "finish") }.isFailure)
        assertTrue(ledger.tailscaleTargetAction(binding, file, "action-one", "finish").getBoolean("released"))
        assertTrue(ledger.enterBirth(pending))
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
        assertTrue(runCatching { ledger.tailscaleTargetAction(binding, file, "action-one", "finish") }.isFailure)
        assertTrue(runCatching { ledger.tailscaleTargetAction(binding, file, "second", "begin") }.isFailure)
    }

    @Test fun membershipCanExcludeOnlyItsOriginalValidationTicketAndUnknownLedgerRemains() {
        val ledger = open()
        val (_, custody) = ScopedNativeFixture.closedOwner(ledger, "original")
        val validation = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        assertTrue(ledger.enterBirth(validation))
        val scope = AndroidTailscaleValidationScope.capture(validation.id, "no-ts", ScopedNativeFixture.validation(validation.id, "no-ts", emptyList()))
        ledger.attachValidation(validation, scope)
        assertTrue(ledger.validationCleanupUnknown(validation))
        val file = ScopedNativeFixture.file("one")
        val rows = ledger.tailscaleTargetAction(custody.wire(), file, "query", "query").getJSONArray("entries")
        val row = (0 until rows.length()).map { rows.getJSONObject(it) }.single { it.getString("nativeTicketId") == validation.id }
        assertFalse(row.getBoolean("related"))
        assertEquals("ValidationCleanupUnknown", row.getString("globalState"))
        assertFalse(ledger.seal("global-fence").coverageComplete)
        assertEquals(AndroidNativeAdmission.State.ValidationCleanupUnknown, ledger.receipt("global-fence").captured.single { it.ticket == validation }.state)
        val other = open()
        val (_, otherCustody) = ScopedNativeFixture.closedOwner(other, "original")
        val runtime = other.reserveOwner(AndroidNativeAdmission.Kind.Speedtest, "same-no-ts-sha")
        assertTrue(other.enterBirth(runtime))
        other.bindTailscaleStore(runtime)
        assertTrue(other.unknown(runtime))
        assertTrue(runCatching { other.tailscaleTargetAction(otherCustody.wire(), file, "query", "query") }.isFailure)
    }

    @Test fun twoConcurrentTargetActionsHaveOneWinnerAndDoNotSealTheLedger() {
        val ledger = open()
        val (_, custody) = ScopedNativeFixture.closedOwner(ledger, "original")
        val binding = custody.wire()
        val winners = java.util.Collections.synchronizedList(mutableListOf<String>())
        val release = CountDownLatch(1)
        val workers = listOf("first", "second").map { action -> Thread {
            check(release.await(2, TimeUnit.SECONDS))
            if (runCatching { ledger.tailscaleTargetAction(binding, ScopedNativeFixture.file("one"), action, "begin") }.isSuccess) winners.add(action)
        }.also { it.start() } }
        release.countDown()
        workers.forEach { it.join(2_000); assertFalse(it.isAlive) }
        assertEquals(1, winners.size)
        ledger.tailscaleTargetAction(binding, ScopedNativeFixture.file("one"), winners.single(), "finish")
        val next = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "next")
        assertTrue(ledger.enterBirth(next))
    }

    @Test fun scopedByteBudgetRetainsOriginalRecordsAndCannotBecomeAGlobalCloseFact() {
        val ledger = open()
        val first = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        val second = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        assertTrue(ledger.admitScopedBytes(first, AndroidNativeAdmission.MAX_SCOPED_BYTES))
        assertTrue(ledger.admitScopedBytes(first, AndroidNativeAdmission.MAX_SCOPED_BYTES))
        assertFalse(ledger.admitScopedBytes(second, 1))
        assertTrue(ledger.admitScopedBytes(first, AndroidNativeAdmission.MAX_SCOPED_BYTES / 2))
        assertTrue(ledger.admitScopedBytes(second, AndroidNativeAdmission.MAX_SCOPED_BYTES / 2))
        assertFalse(ledger.admitScopedBytes(first, AndroidNativeAdmission.MAX_SCOPED_BYTES / 2 + 1))
        assertEquals(AndroidNativeAdmission.State.Reserved, ledger.state(first))
        assertEquals(AndroidNativeAdmission.State.Reserved, ledger.state(second))
        assertTrue(ledger.enterBirth(second))
        assertTrue(ledger.validationCleanupUnknown(second))
        assertEquals(2, ledger.seal("budget-fence").capturedCount)
        assertFalse(ledger.receipt("budget-fence").coverageComplete)
    }

    @Test fun completeUnrelatedRuntimeCensusCanExcludeWithoutProvingItsWriterClosed() {
        val ledger = open()
        val (_, target) = ScopedNativeFixture.closedOwner(ledger, "target")
        val unrelated = ledger.reserveOwner(AndroidNativeAdmission.Kind.Speedtest, "unrelated")
        assertTrue(ledger.enterBirth(unrelated))
        val store = ledger.bindTailscaleStore(unrelated)
        var native = ScopedNativeFixture.export()
        val run = ScopedNativeFixture.run("b", "other", listOf("other"))
        store.invoke("other", { native }) { native = ScopedNativeFixture.export(run) }
        store.closed { ScopedNativeFixture.export(org.json.JSONObject(run.toString()).put("censusComplete", true)) }
        assertTrue(ledger.unknown(unrelated))
        assertEquals(false, store.closedRelation(ScopedNativeFixture.file("one")))
        assertEquals(null, store.closedRelation(ScopedNativeFixture.file("other")))
        val rows = ledger.tailscaleTargetAction(target.wire(), ScopedNativeFixture.file("one"), "query", "query").getJSONArray("entries")
        assertEquals(2, rows.length())
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(unrelated))
    }

    @Test fun replacingTheOriginalSnapshotAccountsItsCurrentBytesInsteadOfAccumulatingReloadCopies() {
        val ledger = open()
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "reloader")
        assertTrue(ledger.enterBirth(ticket))
        val custody = ledger.bindTailscaleStore(ticket, "birth")
        val history = mutableListOf<org.json.JSONObject>()
        var native = ScopedNativeFixture.export()
        repeat(160) { index ->
            val config = "config-$index"
            custody.invoke(config, { native }) {
                history.add(ScopedNativeFixture.run("a", config, listOf("one")).put("runNonce", index.toString(16).padStart(64, '0')))
                native = ScopedNativeFixture.export(*history.toTypedArray())
            }
        }
        assertFalse(custody.wire().getBoolean("historyUnknown"))
        assertEquals(160, custody.wire().getJSONArray("originalObservedRuns").length())
        custody.closed { ScopedNativeFixture.export(*history.map(ScopedNativeFixture::retired).toTypedArray()) }
        assertEquals(true, custody.closedRelation(ScopedNativeFixture.file("one")))
        assertTrue(custody.wire().toString().toByteArray(Charsets.UTF_8).size <= AndroidTailscaleStoreCustody.MAX_BYTES)
    }

    @Test fun originalAttachmentsHitTheByteBudgetWithoutEvictionOrFalseClosure() {
        val ledger = open()
        val stores = mutableListOf<AndroidTailscaleStoreCustody>()
        repeat(12) { owner ->
            val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "large-$owner")
            assertTrue(ledger.enterBirth(ticket))
            val custody = ledger.bindTailscaleStore(ticket)
            var native = ScopedNativeFixture.export()
            val tags = (0 until 512).map { "owner-$owner-node-$it-" + "x".repeat(170) }
            val run = ScopedNativeFixture.run(owner.toString(16), "config-$owner", tags)
            custody.invoke("config-$owner", { native }) { native = ScopedNativeFixture.export(run) }
            custody.closed { ScopedNativeFixture.export(ScopedNativeFixture.retired(run)) }
            assertTrue(ledger.unknown(ticket))
            assertTrue(custody.wire().toString().toByteArray(Charsets.UTF_8).size <= AndroidTailscaleStoreCustody.MAX_BYTES)
            stores.add(custody)
        }
        assertFalse(stores.first().wire().getBoolean("historyUnknown"))
        assertTrue(stores.last().wire().getBoolean("historyUnknown"))
        assertEquals(true, stores.first().closedRelation(stores.first().wire().getJSONArray("originalObservedRuns").getJSONObject(0).getJSONArray("scopes").getJSONObject(0).getString("stateFile")))
        assertEquals(null, stores.last().closedRelation(ScopedNativeFixture.file("one")))
        val receipt = ledger.seal("large-fence")
        assertEquals(12, receipt.capturedCount)
        assertTrue(receipt.captured.all { it.state == AndroidNativeAdmission.State.Unknown })
        assertFalse(receipt.coverageComplete)
    }

    @Test fun queryRejectsMultipleIndividuallyBoundedRowsBeforeInstallingAnAction() {
        val ledger = open()
        val stores = (0 until 3).map { owner ->
            val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "row-$owner")
            assertTrue(ledger.enterBirth(ticket))
            val custody = ledger.bindTailscaleStore(ticket)
            var native = ScopedNativeFixture.export()
            val tags = (0 until 512).map { "owner-$owner-node-$it-" + "x".repeat(110) }
            val run = ScopedNativeFixture.run(owner.toString(16), "config-$owner", tags)
            custody.invoke("config-$owner", { native }) { native = ScopedNativeFixture.export(run) }
            custody.closed { ScopedNativeFixture.export(ScopedNativeFixture.retired(run)) }
            assertTrue(ledger.unknown(ticket))
            assertFalse(custody.wire().getBoolean("historyUnknown"))
            custody
        }
        val original = stores.first().wire()
        val file = original.getJSONArray("originalObservedRuns").getJSONObject(0).getJSONArray("scopes").getJSONObject(0).getString("stateFile")
        assertTrue(runCatching { ledger.tailscaleTargetAction(original, file, "oversized-family", "begin") }.isFailure)
        val successor = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "next")
        assertTrue(ledger.enterBirth(successor))
    }

    @Test fun coldWarmReservesWholeFamilyAndOnlyOneOriginalLoginAndChildCanBirth() {
        val ledger = open()
        val config = ScopedNativeFixture.warmConfig
        val tuple = ScopedNativeFixture.warm(ledger, "warm-one")
        val first = ledger.beginWarm(tuple)
        assertTrue(first.getBoolean("held"))
        assertFalse(first.getBoolean("claimed"))
        assertTrue(first.similar(ledger.beginWarm(tuple))) // Lost begin response recovers the same ticket.
        val unrelated = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "foreign-main")
        val foreignCheck = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        assertFalse(ledger.enterBirth(unrelated))
        assertFalse(ledger.enterBirth(foreignCheck))
        assertTrue(ledger.cancelBeforeBirth(unrelated))
        assertTrue(ledger.cancelBeforeBirth(foreignCheck))
        val warm = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "warm-one")
        assertEquals(first.getString("nativeTicketId"), warm.id)
        assertTrue(runCatching { ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "warm-one") }.isFailure)
        assertFalse(ledger.enterBirth(warm))
        ledger.bindWarmConfig(warm, config)
        val child = ledger.reserveWarmValidation(warm)
        assertTrue(runCatching { ledger.reserveWarmValidation(warm) }.isFailure)
        assertFalse(ledger.enterBirth(child))
        assertTrue(ledger.enterBirth(warm))
        assertTrue(ledger.enterBirth(child))
        ledger.attachValidation(child, AndroidTailscaleValidationScope.capture(child.id, config,
            ScopedNativeFixture.validation(child.id, config, listOf("one"), true)))
        assertTrue(ledger.validationCleanupUnknown(child))
        val store = ledger.bindTailscaleStore(warm)
        var native = ScopedNativeFixture.export()
        val run = ScopedNativeFixture.run("b", config, listOf("one"))
        store.invoke(config, { native }) { native = ScopedNativeFixture.export(run) }
        store.closed { ScopedNativeFixture.export(ScopedNativeFixture.retired(run)) }
        assertTrue(ledger.unknown(warm))
        val held = ledger.readWarm(tuple)
        assertTrue(held.getBoolean("held"))
        assertTrue(held.has("family"))
        assertEquals(3, held.getJSONObject("family").getJSONArray("entries").let { rows ->
            (0 until rows.length()).count { rows.getJSONObject(it).getBoolean("related") }
        })
        assertTrue(runCatching { ledger.tailscaleTargetAction(store.wire(), ScopedNativeFixture.file("one"), "warm-action", "finish") }.isFailure)
        assertTrue(ledger.readWarm(tuple).getBoolean("held"))
        assertTrue(ledger.finishWarm(tuple).getBoolean("released"))
        assertTrue(ledger.finishWarm(tuple).getBoolean("released"))
        assertFalse(ledger.readWarm(tuple).getBoolean("held"))
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(warm))
        assertEquals(AndroidNativeAdmission.State.ValidationCleanupUnknown, ledger.state(child))
        val next = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "successor")
        assertTrue(ledger.enterBirth(next))
    }

    @Test fun claimedPreparationCannotBeGuessedUnbornAndActualQueuedChildIsCancelledPrecisely() {
        val ledger = open(); val tuple = ScopedNativeFixture.warm(ledger, "pending-warm")
        ledger.beginWarm(tuple)
        val owner = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "pending-warm")
        assertTrue(ledger.readWarm(tuple).getBoolean("childRegistrationOpen"))
        assertTrue(runCatching { ledger.finishWarm(tuple) }.isFailure)
        ledger.bindWarmConfig(owner, ScopedNativeFixture.warmConfig)
        val child = ledger.reserveWarmValidation(owner)
        assertEquals(child.id, ledger.readWarm(tuple).getString("childNativeTicketId"))
        assertTrue(ledger.finishWarm(tuple).getBoolean("released"))
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(owner))
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(child))
        assertFalse(ledger.enterBirth(owner)); assertFalse(ledger.enterBirth(child))
        assertTrue(runCatching { ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "pending-warm") }.isFailure)
    }

    @Test fun finishBeforeBeginRecordsOriginalConsumedIdentityAndRejectsLateOrReplayedBegin() {
        repeat(20) { turn ->
            val ledger = open(); val tuple = ScopedNativeFixture.warm(ledger, "never-started-$turn")
            val released = ledger.finishWarm(tuple)
            assertTrue(released.getBoolean("released")); assertTrue(released.getBoolean("beforeBirth"))
            assertEquals("", released.getString("nativeTicketId"))
            assertTrue(ledger.finishWarm(tuple).similar(released))
            assertTrue(runCatching { ledger.beginWarm(tuple) }.isFailure)
            assertTrue(runCatching { ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, tuple.getString("logicalInstanceId")) }.isFailure)
            assertEquals(1, ledger.metadataUsage().consumedOwners)
            assertEquals(1, ledger.metadataUsage().entries)
            assertFalse(ledger.metadataUsage().capacityClosed)
            assertEquals(AndroidNativeAdmission.State.ValidationCleanupUnknown,
                ledger.seal("proof-$turn").captured.single().state)
        }
    }

    @Test fun concurrentFinishAndLostBeginHaveOneOriginalIntentAndNoLateBirth() {
        repeat(20) { turn ->
            val ledger = open(); val tuple = ScopedNativeFixture.warm(ledger, "racing-$turn")
            val release = CountDownLatch(1); val finishError = AtomicReference<Throwable?>()
            val begin = Thread { check(release.await(2, TimeUnit.SECONDS)); runCatching { ledger.beginWarm(tuple) } }
            val finish = Thread { check(release.await(2, TimeUnit.SECONDS)); finishError.set(runCatching { ledger.finishWarm(tuple) }.exceptionOrNull()) }
            begin.start(); finish.start(); release.countDown()
            listOf(begin, finish).forEach { it.join(2_000); assertFalse(it.isAlive) }
            assertEquals(null, finishError.get())
            assertTrue(ledger.finishWarm(tuple).getBoolean("released"))
            assertTrue(runCatching { ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, tuple.getString("logicalInstanceId")) }.isFailure)
            runCatching { ledger.readWarm(tuple) }.getOrNull()?.let { assertFalse(it.getBoolean("held")) }
        }
    }

    @Test fun rejectedWholeFamilyOrPermanentCapacityStillAllowsExactBeforeBirthIntentCancellation() {
        for (reason in listOf("unknown", "foreign-action", "capacity")) {
            val ledger = if (reason == "capacity") bounded(3) else open()
            val tuple = ScopedNativeFixture.warm(ledger, "rejected-$reason")
            when (reason) {
                "unknown" -> ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "unknown-owner")
                "foreign-action" -> {
                    val (_, original) = ScopedNativeFixture.closedOwner(ledger, "other")
                    ledger.tailscaleTargetAction(original.wire(), ScopedNativeFixture.file("one"), "held-elsewhere", "begin")
                }
            }
            assertTrue(runCatching { ledger.beginWarm(tuple) }.isFailure)
            val released = ledger.finishWarm(tuple)
            assertTrue(released.getBoolean("beforeBirth")); assertTrue(released.getBoolean("released"))
            assertTrue(runCatching { ledger.beginWarm(tuple) }.isFailure)
            assertTrue(runCatching { ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, tuple.getString("logicalInstanceId")) }.isFailure)
            if (reason == "foreign-action") assertFalse(ledger.enterBirth(ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)))
        }
    }

    @Test fun maybeBornWithoutRuntimeStoreRemainsManageableButNeverReleasesItsReservation() {
        val ledger = open(); val tuple = ScopedNativeFixture.warm(ledger, "factory-failed")
        ledger.beginWarm(tuple)
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "factory-failed")
        ledger.bindWarmConfig(ticket, ScopedNativeFixture.warmConfig)
        val child = ledger.reserveWarmValidation(ticket)
        assertTrue(ledger.enterBirth(ticket)); assertTrue(ledger.cancelBeforeBirth(child)); assertTrue(ledger.unknown(ticket))
        repeat(2) {
            val management = ledger.readWarm(tuple)
            assertEquals(ticket.id, management.getString("nativeTicketId"))
            assertEquals("Unknown", management.getString("globalState"))
            assertTrue(management.getBoolean("held")); assertFalse(management.has("runtime")); assertFalse(management.has("family"))
            ledger.beginWarmClose(tuple, "same-close")
            assertTrue(runCatching { ledger.finishWarm(tuple) }.isFailure)
        }
        assertTrue(runCatching { ledger.beginWarmClose(tuple, "successor-close") }.isFailure)
        val pending = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "must-wait")
        assertFalse(ledger.enterBirth(pending))
    }

    @Test fun warmActualConfigCannotIntroduceAnAuthKeyOrChangeItsOriginalDigest() {
        for (config in listOf("{}", ScopedNativeFixture.warmConfig.replace("\"tag\":\"one\"", "\"tag\":\"two\""),
            "{\"endpoints\":[{\"type\":\"tailscale\",\"auth_key\":\"synthetic-only\"}]}")) {
            val ledger = open(); val tuple = ScopedNativeFixture.warm(ledger, "no-auth")
            ledger.beginWarm(tuple)
            val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "no-auth")
            assertTrue(runCatching { ledger.bindWarmConfig(ticket, config) }.isFailure)
            assertFalse(ledger.enterBirth(ticket))
            ledger.finishWarmPreparation(ticket)
            assertTrue(ledger.finishWarm(tuple).getBoolean("released"))
        }
    }

    @Test fun warmCourierViewCannotMutateTheOriginalEntryTupleOrRebindItsReservation() {
        val ledger = open(); val tuple = ScopedNativeFixture.warm(ledger, "immutable-warm")
        val view = ledger.beginWarm(tuple)
        val ticket = view.getString("nativeTicketId")
        view.getJSONObject("tuple").put("actionRequestId", "changed-action").put("stateFile", ScopedNativeFixture.file("other"))
            .getJSONObject("validation").getJSONObject("validation").put("configDigest", "e".repeat(64))
        val original = ledger.readWarm(tuple)
        assertEquals(ticket, original.getString("nativeTicketId"))
        assertTrue(tuple.similar(original.getJSONObject("tuple")))
        assertTrue(original.getBoolean("held"))
        assertTrue(runCatching { ledger.readWarm(view.getJSONObject("tuple")) }.isFailure)
        assertTrue(ledger.finishWarm(tuple).getBoolean("released"))
    }

}


/** Synthetic native boundary data; the actual ledger/custody/parser run, no SDK is invoked. */
internal object ScopedNativeFixture {
    const val warmConfig = "{\"endpoints\":[{\"type\":\"tailscale\",\"tag\":\"one\",\"state_directory\":\"/data/polaris/one\"}]}"
    fun warm(ledger: AndroidNativeAdmission, id: String): org.json.JSONObject {
        val validation = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        check(ledger.enterBirth(validation))
        ledger.attachValidation(validation, AndroidTailscaleValidationScope.capture(validation.id, warmConfig,
            validation(validation.id, warmConfig, listOf("one"), true)))
        check(ledger.validationCleanupUnknown(validation))
        return org.json.JSONObject().put("contractVersion", "polaris-android-ts-cold-warm-v1")
            .put("validation", ledger.validationOwner(validation)).put("stateFile", file("one"))
            .put("actionRequestId", "warm-action").put("logicalInstanceId", id)
    }
    fun digest(config: String) = AndroidTailscaleStoreCustody.digest(config)
    fun file(tag: String) = "/data/polaris/$tag/tailscaled.state"
    fun node(tag: String, terminal: Boolean = false) = org.json.JSONObject()
        .put("tag", tag).put("stateDirectory", "/data/polaris/$tag").put("stateFile", file(tag))
        .put("writerState", if (terminal) "NoStoreConstruction" else "Unknown")
        .put("stateFileState", "Missing").put("stateFileRevision", "")
        .put("profileState", "None").put("profileFingerprint", "")
    fun run(nonce: String, config: String, tags: List<String>, terminal: Boolean = false) = org.json.JSONObject()
        .put("runNonce", nonce.repeat(64)).put("configDigest", digest(config))
        .put("terminal", if (terminal) "NoStoreConstruction" else "Unknown").put("censusComplete", terminal)
        .put("nodes", org.json.JSONArray(tags.map { node(it, terminal) }))
    fun retired(run: org.json.JSONObject): org.json.JSONObject = org.json.JSONObject(run.toString()).also {
        it.put("terminal", "NoStoreConstruction").put("censusComplete", true)
        val nodes = it.getJSONArray("nodes")
        for (index in 0 until nodes.length()) nodes.getJSONObject(index).put("writerState", "NoStoreConstruction")
    }
    fun export(vararg runs: org.json.JSONObject) = org.json.JSONObject()
        .put("contractVersion", "polaris-ts-auth-writer-retirement-v1").put("globalCleanupEvidence", "CleanupUnknown")
        .put("instances", org.json.JSONArray(runs.toList())).toString()
    fun closedOwner(ledger: AndroidNativeAdmission, logical: String): Pair<AndroidNativeAdmission.Ticket, AndroidTailscaleStoreCustody> {
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, logical)
        check(ledger.enterBirth(ticket))
        val custody = ledger.bindTailscaleStore(ticket)
        var native = export()
        val run = run("a", "original", listOf("one"))
        custody.invoke("original", { native }) { native = export(run) }
        custody.closed { export(retired(run)) }
        check(ledger.unknown(ticket))
        return ticket to custody
    }
    class ValidationResult(private val request: String, private val config: String,
        var member: String, var retirement: String, var validationState: String = "Accepted") {
        fun getContractVersion() = "polaris-validation-v1"
        fun getRequestID() = request
        fun getConfigDigest() = digest(config)
        var cleanupState = "CleanupUnknown"
        fun getCleanup() = cleanupState
        fun getValidation() = validationState
        fun getTailscaleStoreMembership() = member
        fun getTailscaleStoreRetirement() = retirement
    }
    fun validation(request: String, config: String, tags: List<String>, writerTerminal: Boolean = false): ValidationResult {
        val run = run("d", config, tags, writerTerminal)
        val targets = org.json.JSONArray(tags.map { tag -> org.json.JSONObject()
            .put("tag", tag).put("stateDirectory", "/data/polaris/$tag").put("stateFile", file(tag)) })
        val member = org.json.JSONObject().put("contractVersion", "polaris-ts-store-target-membership-v1")
            .put("requestID", request).put("configDigest", digest(config)).put("runNonce", "d".repeat(64))
            .put("membershipState", "Complete").put("targets", targets).toString()
        return ValidationResult(request, config, member, export(run))
    }
}
