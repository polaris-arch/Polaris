package com.polaris2.app.vpn

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class AndroidNativeAdmissionTest {
    private fun bounded(limit: Int) = AndroidNativeAdmission("capacity-process", maxMetadataRecords = limit).also {
        it.bootstrap(RequiredMarkerProof.Absent)
    }

    @Test fun runtimeWriterHistoryRemainsWithItsExactBirthAndOriginalRelease() {
        val ledger = bounded(20)
        val main = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "main")
        val custody = ledger.bindTailscaleStore(main, "birth")
        val writer = runtimeWriterScope(1)
        assertFalse(ledger.retainRuntimeWriter(main, "birth", writer))
        assertTrue(ledger.enterBirth(main))
        var native = ScopedNativeFixture.export()
        val cold = ScopedNativeFixture.run("c", "cold", listOf("cold"))
        custody.invoke("cold", { native }) { native = ScopedNativeFixture.export(cold) }
        val oldColdView = custody.wire().toString()
        assertFalse(ledger.retainRuntimeWriter(main, "other-birth", writer))
        assertTrue(ledger.retainRuntimeWriter(main, "birth", writer))
        assertEquals(oldColdView, custody.wire().toString())
        assertEquals(AndroidNativeAdmission.RuntimeWriterOutcome.Held, ledger.runtimeWriterHistory(main, "birth", writer)?.outcome)
        assertFalse(ledger.retainRuntimeWriter(main, "birth", writer.copy(runNonce = "d".repeat(64))))
        assertFalse(ledger.settleRuntimeWriter(main, "birth", writer.copy(generation = 8), true))

        val successor = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "successor")
        ledger.bindTailscaleStore(successor, "next-birth")
        assertTrue(ledger.enterBirth(successor))
        assertTrue(ledger.retainRuntimeWriter(successor, "next-birth", writer))
        val noAcquisition = runtimeWriterScope(2)
        assertTrue(ledger.retainRuntimeWriter(main, "birth", noAcquisition))
        assertTrue(ledger.unknown(main))
        ledger.seal("stop")
        assertFalse(ledger.retainRuntimeWriter(main, "birth", runtimeWriterScope(3)))
        assertTrue(ledger.settleRuntimeWriter(main, "birth", writer, true))
        assertTrue(ledger.settleRuntimeWriter(main, "birth", noAcquisition, false))
        assertFalse(ledger.settleRuntimeWriter(main, "birth", writer, false))
        assertEquals(AndroidNativeAdmission.RuntimeWriterOutcome.ClosedConfirmed, ledger.runtimeWriterHistory(main, "birth", writer)?.outcome)
        assertEquals(AndroidNativeAdmission.RuntimeWriterOutcome.NoAcquisition, ledger.runtimeWriterHistory(main, "birth", noAcquisition)?.outcome)
        assertEquals(AndroidNativeAdmission.RuntimeWriterOutcome.Held, ledger.runtimeWriterHistory(successor, "next-birth", writer)?.outcome)
        assertEquals(null, ledger.runtimeWriterHistory(successor, "birth", writer))

        // A late export that omits the runtime writer cannot upgrade the old cold deletion proof.
        custody.closed { ScopedNativeFixture.export(ScopedNativeFixture.retired(cold)) }
        assertTrue(custody.wire().getBoolean("historyUnknown"))
        assertEquals(null, custody.closedRelation(ScopedNativeFixture.file("cold")))
        assertEquals(null, custody.closedRelation("${writer.stateDirectory}/tailscaled.state"))
    }

    @Test fun runtimeWriterHistoryReservesBoundedBytesWithoutEvictingClosedRows() {
        val ledger = bounded(20)
        val main = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "main")
        val sibling = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "login")
        ledger.bindTailscaleStore(main, "birth")
        val original = runtimeWriterScope(1)
        fun canRetain() = ledger.canRetainRuntimeWriter(main, "birth", original.incarnation, original.tag,
            original.stateDirectory, original.taildropDirectory, RuntimeTailscaleStatePaths(original.stateDirectory, original.stateDirectory))
        assertFalse(canRetain())
        assertTrue(ledger.enterBirth(main))
        assertTrue(canRetain())
        assertTrue(ledger.admitScopedBytes(sibling, AndroidNativeAdmission.MAX_SCOPED_BYTES))
        assertFalse(canRetain())
        assertFalse(ledger.retainRuntimeWriter(main, "birth", original))
        assertEquals(null, ledger.runtimeWriterHistory(main, "birth", original))
        assertTrue(ledger.admitScopedBytes(sibling, 0))
        assertTrue(canRetain())
        for (sequence in 1..AndroidTailscaleStoreCustody.MAX_RUNS) {
            val writer = runtimeWriterScope(sequence)
            assertTrue(ledger.retainRuntimeWriter(main, "birth", writer))
            assertTrue(ledger.settleRuntimeWriter(main, "birth", writer, false))
        }
        assertFalse(canRetain())
        assertFalse(ledger.retainRuntimeWriter(main, "birth", runtimeWriterScope(AndroidTailscaleStoreCustody.MAX_RUNS + 1)))
        assertEquals(AndroidNativeAdmission.RuntimeWriterOutcome.NoAcquisition,
            ledger.runtimeWriterHistory(main, "birth", runtimeWriterScope(1))?.outcome)
        assertFalse(ledger.admitScopedBytes(sibling, AndroidNativeAdmission.MAX_SCOPED_BYTES))
    }

    @Test fun terminalRuntimeHistoryReadsOnlyItsCompleteOriginalWireBinding() {
        val ledger = bounded(20)
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "main")
        ledger.bindTailscaleStore(ticket, "birth")
        assertTrue(ledger.enterBirth(ticket))
        val first = runtimeWriterScope(1).copy(resourceSequence = Long.MIN_VALUE, generation = -1L)
        val second = runtimeWriterScope(2)
        fun binding(scope: RuntimeTailscaleWriterScope) = JSONObject()
            .put("contractVersion", "polaris-android-runtime-ts-writer-v1")
            .put("processNonce", ledger.processNonce).put("nativeTicketId", ticket.id)
            .put("runId", ticket.logicalId).put("birthNonce", "birth").put("scope", scope.json())
        fun read(value: JSONObject) = AndroidRuntimeTailscaleBridge.readHistory(value, ledger)
        val original = binding(first)
        assertTrue(runCatching { read(original) }.isFailure)
        assertTrue(ledger.retainRuntimeWriter(ticket, "birth", first))
        assertTrue(ledger.retainRuntimeWriter(ticket, "birth", second))
        assertTrue(runCatching { read(original) }.isFailure)
        assertTrue(ledger.unknown(ticket))
        assertTrue(ledger.settleRuntimeWriter(ticket, "birth", first, true))
        assertTrue(ledger.settleRuntimeWriter(ticket, "birth", second, false))

        val confirmed = read(original)
        assertEquals("ClosedConfirmed", confirmed.getString("outcome"))
        assertEquals(original.toString(), confirmed.getJSONObject("original").toString())
        val scope = confirmed.getJSONObject("original").getJSONObject("scope")
        assertEquals("9223372036854775808", scope.getString("resourceSequence"))
        assertEquals("18446744073709551615", scope.getString("generation"))
        assertEquals("NoAcquisition", read(binding(second)).getString("outcome"))

        for (field in listOf("processNonce", "nativeTicketId", "runId", "birthNonce")) {
            assertTrue(runCatching { read(binding(first).put(field, "wrong")) }.isFailure)
        }
        val wrongDirectory = binding(first.copy(taildropDirectory = "/different/Taildrop"))
        assertTrue(runCatching { read(wrongDirectory) }.isFailure)
        val wrongNumber = binding(first).also { it.getJSONObject("scope").put("generation", "018446744073709551615") }
        assertTrue(runCatching { read(wrongNumber) }.isFailure)
        assertTrue(runCatching { read(binding(first).put("extra", true)) }.isFailure)
    }

    @Test fun confirmedRuntimeHistoryKeepsColdTargetsAvailableAndBlocksOnlyItsWarmAliases() {
        val ledger = bounded(20)
        val main = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "main")
        val custody = ledger.bindTailscaleStore(main, "birth")
        assertTrue(ledger.enterBirth(main))
        val oldCold = ScopedNativeFixture.run("c", "cold", listOf("old"))
        var native = ScopedNativeFixture.export()
        custody.invoke("cold", { native }) { native = ScopedNativeFixture.export(oldCold) }
        val coldBinding = custody.wire()
        val writer = runtimeWriterScope(1)
        val state = RuntimeTailscaleStatePaths("/canonical/tailscale/node-1", writer.stateDirectory)
        val aborted = runtimeWriterScope(2)
        assertTrue(ledger.retainRuntimeWriter(main, "birth", writer, state))
        assertTrue(ledger.retainRuntimeWriter(main, "birth", aborted))
        assertTrue(ledger.settleRuntimeWriter(main, "birth", writer, true))
        assertTrue(ledger.settleRuntimeWriter(main, "birth", aborted, false))
        assertTrue(coldBinding.similar(custody.wire()))

        custody.closed { ScopedNativeFixture.export(ScopedNativeFixture.retired(oldCold), runtimeStoreRun(writer, state.canonical, true)) }
        assertTrue(ledger.unknown(main))
        assertTrue(custody.matches(coldBinding))
        assertFalse(custody.wire().getBoolean("historyUnknown"))
        val coldProof = custody.wire().getString("storeRetirement")
        assertEquals(1, AndroidTailscaleStoreCustody.parseRuns(coldProof).size)
        assertTrue(AndroidTailscaleStoreCustody.terminalComplete(coldProof))
        assertEquals(true, custody.closedRelation(ScopedNativeFixture.file("old")))
        assertEquals(false, custody.closedRelation(ScopedNativeFixture.file("unrelated")))
        for (file in state.files) {
            assertEquals(null, custody.closedRelation(file))
            assertTrue(runCatching { ledger.tailscaleTargetAction(custody.wire(), file, "warm", "begin") }.isFailure)
        }

        val (_, unrelated) = ScopedNativeFixture.closedOwner(ledger, "unrelated")
        val file = ScopedNativeFixture.file("one")
        val opened = ledger.tailscaleTargetAction(unrelated.wire(), file, "cold-target", "begin").getJSONArray("entries")
        val mainRow = (0 until opened.length()).map(opened::getJSONObject).single { it.getString("nativeTicketId") == main.id }
        assertEquals(false, mainRow.getBoolean("related"))
        assertTrue(ledger.tailscaleTargetAction(unrelated.wire(), file, "cold-target", "finish").getBoolean("released"))
    }

    @Test fun strictReloadKeepsWarmHistorySeparateFromTheReplacementColdRun() {
        val ledger = bounded(20)
        val main = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "main")
        val custody = ledger.bindTailscaleStore(main, "birth")
        assertTrue(ledger.enterBirth(main))
        val oldCold = ScopedNativeFixture.run("c", "cold", listOf("old"))
        val newCold = ScopedNativeFixture.run("d", "reload", listOf("replacement"))
        val warm = runtimeWriterScope(1)
        val unused = runtimeWriterScope(2)
        var native = ScopedNativeFixture.export()
        custody.invoke("cold", { native }) { native = ScopedNativeFixture.export(oldCold) }
        assertTrue(ledger.retainRuntimeWriter(main, "birth", warm))
        assertTrue(ledger.retainRuntimeWriter(main, "birth", unused))
        native = ScopedNativeFixture.export(oldCold, runtimeStoreRun(warm, warm.stateDirectory, false),
            runtimeStoreRun(unused, null, false))
        custody.invoke("reload", { native }) {
            assertTrue(ledger.settleRuntimeWriter(main, "birth", warm, true))
            assertTrue(ledger.settleRuntimeWriter(main, "birth", unused, false))
            native = ScopedNativeFixture.export(ScopedNativeFixture.retired(oldCold), runtimeStoreRun(warm, warm.stateDirectory, true),
                runtimeStoreRun(unused, null, true), newCold)
        }
        val active = custody.wire()
        assertFalse(active.getBoolean("historyUnknown"))
        assertFalse(active.has("currentStartRunNonces"))
        assertFalse(custody.hasConfirmedCurrentStart())
        assertEquals(ScopedNativeFixture.digest("reload"), active.getString("actualConfigDigest"))
        assertEquals(2, active.getJSONArray("originalObservedRuns").length())
        custody.closed {
            ScopedNativeFixture.export(ScopedNativeFixture.retired(oldCold), runtimeStoreRun(warm, warm.stateDirectory, true),
                runtimeStoreRun(unused, null, true), ScopedNativeFixture.retired(newCold))
        }
        assertTrue(custody.matches(active))
        assertEquals(true, custody.closedRelation(ScopedNativeFixture.file("replacement")))
        assertEquals(null, custody.closedRelation("${warm.stateDirectory}/tailscaled.state"))
    }

    @Test fun typedStartupObservesOriginalParentAndInitialPhysicalChild() {
        val ledger = bounded(20)
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "main")
        val custody = ledger.bindTailscaleStore(ticket, "birth")
        assertTrue(ledger.enterBirth(ticket))
        assertFalse(custody.hasConfirmedCurrentStart())
        val rawConfig = "original input"
        val parent = ScopedNativeFixture.run("a", rawConfig, emptyList())
        val child = ScopedNativeFixture.run("b", "frozen construction with resolved inputs", listOf("first"))
        var native = ScopedNativeFixture.export()
        custody.invoke(rawConfig, { native }, startupReceipt = { it }) {
            native = ScopedNativeFixture.export(parent, child)
            ScopedNativeFixture.cohort(parent, child)
        }
        val active = custody.wire()
        assertFalse(active.getBoolean("historyUnknown"))
        assertTrue(custody.hasConfirmedCurrentStart())
        assertEquals(ScopedNativeFixture.digest(rawConfig), active.getString("actualConfigDigest"))
        assertEquals(listOf("b".repeat(64), "a".repeat(64)), active.getJSONArray("currentStartRunNonces").let {
            listOf(it.getString(0), it.getString(1))
        })
        assertEquals("a".repeat(64), active.getJSONArray("originalObservedRuns").getJSONObject(1).getString("runNonce"))
        val wrongOrder = JSONObject(active.toString()).put("currentStartRunNonces", org.json.JSONArray(listOf("a".repeat(64), "b".repeat(64))))
        val withoutGroup = JSONObject(active.toString()).also { it.remove("currentStartRunNonces") }
        assertFalse(custody.matches(wrongOrder))
        assertFalse(custody.matches(withoutGroup))
        custody.closed { ScopedNativeFixture.export(ScopedNativeFixture.retired(child), ScopedNativeFixture.retired(parent)) }
        assertTrue(custody.matches(active))
        assertFalse(custody.hasConfirmedCurrentStart())
        assertEquals(true, custody.closedRelation(ScopedNativeFixture.file("first")))
        assertEquals(false, custody.closedRelation(ScopedNativeFixture.file("unrelated")))
    }

    @Test fun typedReloadRetainsWarmCloseAndAdmitsItsReplacementPhysicalWriterAsInitial() {
        val ledger = bounded(20)
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "main")
        val custody = ledger.bindTailscaleStore(ticket, "birth")
        assertTrue(ledger.enterBirth(ticket))
        val parent = ScopedNativeFixture.run("a", "empty graph", emptyList())
        var native = ScopedNativeFixture.export()
        custody.invoke("empty graph", { native }, startupReceipt = { it }) {
            native = ScopedNativeFixture.export(parent)
            ScopedNativeFixture.cohort(parent)
        }
        val warm = runtimeWriterScope(1)
        assertTrue(ledger.retainRuntimeWriter(ticket, "birth", warm))
        native = ScopedNativeFixture.export(parent, runtimeStoreRun(warm, warm.stateDirectory, false))
        val replacementParent = ScopedNativeFixture.run("b", "configuration including warm endpoint", emptyList())
        val replacementChild = runtimeStoreRun(warm, warm.stateDirectory, false)
            .put("runNonce", "c".repeat(64)).put("configDigest", ScopedNativeFixture.digest("replacement physical input"))
        val retiredParent = ScopedNativeFixture.retired(parent)
        val retiredWarm = runtimeStoreRun(warm, warm.stateDirectory, true)
        custody.invoke("configuration including warm endpoint", { native }, startupReceipt = { it }) {
            assertTrue(ledger.settleRuntimeWriter(ticket, "birth", warm, true))
            native = ScopedNativeFixture.export(retiredParent, retiredWarm, replacementParent, replacementChild)
            ScopedNativeFixture.cohort(replacementParent, replacementChild)
        }
        val active = custody.wire()
        assertFalse(active.getBoolean("historyUnknown"))
        assertEquals(3, active.getJSONArray("originalObservedRuns").length())
        assertEquals(2, active.getJSONArray("currentStartRunNonces").length())
        assertEquals("c".repeat(64), active.getJSONArray("currentStartRunNonces").getString(0))
        assertEquals(AndroidNativeAdmission.RuntimeWriterOutcome.ClosedConfirmed, ledger.runtimeWriterHistory(ticket, "birth", warm)?.outcome)
        custody.closed {
            ScopedNativeFixture.export(retiredParent, retiredWarm,
                ScopedNativeFixture.retired(replacementChild), ScopedNativeFixture.retired(replacementParent))
        }
        assertTrue(custody.matches(active))
        assertFalse(custody.wire().getBoolean("historyUnknown"))
        assertEquals(null, custody.closedRelation("${warm.stateDirectory}/tailscaled.state"))
        assertEquals(false, custody.closedRelation(ScopedNativeFixture.file("unrelated")))
    }

    @Test fun typedEmptyReloadRetainsBothEarlierEmptyParentsEvenWithTheSameInput() {
        val ledger = bounded(20)
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "main")
        val custody = ledger.bindTailscaleStore(ticket, "birth")
        assertTrue(ledger.enterBirth(ticket))
        val config = "unchanged empty graph"
        val history = mutableListOf<JSONObject>()
        var native = ScopedNativeFixture.export()
        for (nonce in listOf("a", "b", "c")) {
            val parent = ScopedNativeFixture.run(nonce, config, emptyList())
            custody.invoke(config, { native }, startupReceipt = { it }) {
                native = ScopedNativeFixture.export(*(history + parent).toTypedArray())
                ScopedNativeFixture.cohort(parent)
            }
            history.add(ScopedNativeFixture.retired(parent))
        }
        val active = custody.wire()
        assertFalse(active.getBoolean("historyUnknown"))
        assertEquals(3, active.getJSONArray("originalObservedRuns").length())
        assertEquals("c".repeat(64), active.getJSONArray("currentStartRunNonces").getString(0))
        custody.closed { ScopedNativeFixture.export(*history.toTypedArray()) }
        assertTrue(custody.matches(active))
        assertEquals(false, custody.closedRelation(ScopedNativeFixture.file("unrelated")))
    }

    @Test fun typedStartupCannotAdoptAParallelForeignScopeOrFallBackFromInvalidCohort() {
        for (scenario in listOf("foreign parallel row", "incomplete cohort", "wrong child digest",
            "wrong child scope", "empty reply", "native exception")) {
            val ledger = bounded(20)
            val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "main")
            val custody = ledger.bindTailscaleStore(ticket, "birth")
            assertTrue(ledger.enterBirth(ticket))
            val foreign = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "parallel")
            ledger.bindTailscaleStore(foreign, "other-birth")
            assertTrue(ledger.enterBirth(foreign))
            val foreignScope = runtimeWriterScope(2).copy(configDigest = ScopedNativeFixture.digest("configuration"))
            assertTrue(ledger.retainRuntimeWriter(foreign, "other-birth", foreignScope))
            val parent = ScopedNativeFixture.run("a", "configuration", emptyList())
            val child = ScopedNativeFixture.run("b", "frozen", listOf("first"))
            val foreignRow = runtimeStoreRun(foreignScope, foreignScope.stateDirectory, false)
            var native = ScopedNativeFixture.export()
            var calls = 0
            val nativeFailure = IllegalStateException("original native result is unknown")
            val result = runCatching {
                custody.invoke("configuration", { native }, startupReceipt = { it }) {
                    calls++
                    native = when (scenario) {
                        "foreign parallel row" -> ScopedNativeFixture.export(parent, child, foreignRow)
                        "wrong child digest" -> ScopedNativeFixture.export(parent, ScopedNativeFixture.run("b", "different frozen", listOf("first")))
                        "wrong child scope" -> ScopedNativeFixture.export(parent, ScopedNativeFixture.run("b", "frozen", listOf("other")))
                        else -> ScopedNativeFixture.export(parent)
                    }
                    when (scenario) {
                        "native exception" -> throw nativeFailure
                        "empty reply" -> ""
                        "incomplete cohort" -> JSONObject(ScopedNativeFixture.cohort(parent)).put("censusComplete", false).toString()
                        else -> ScopedNativeFixture.cohort(parent, child)
                    }
                }
            }
            assertEquals(scenario, 1, calls)
            assertEquals(scenario, nativeFailure.takeIf { scenario == "native exception" }, result.exceptionOrNull())
            val active = custody.wire()
            assertTrue(scenario, active.getBoolean("historyUnknown"))
            assertFalse(scenario, active.has("currentStartRunNonces"))
            assertFalse(scenario, custody.hasConfirmedCurrentStart())
            assertEquals(scenario, 0, active.getJSONArray("originalObservedRuns").length())
        }
    }

    @Test fun terminalProjectionRejectsUnmatchedOrIncompleteNativeRows() {
        for (scenario in listOf("foreign-empty", "wrong-path", "wrong-digest", "unconfirmed", "no-acquisition-with-node")) {
            val ledger = bounded(20)
            val main = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "main")
            val custody = ledger.bindTailscaleStore(main, "birth")
            assertTrue(ledger.enterBirth(main))
            val cold = ScopedNativeFixture.run("c", "cold", listOf("old"))
            var native = ScopedNativeFixture.export()
            custody.invoke("cold", { native }) { native = ScopedNativeFixture.export(cold) }
            val warm = runtimeWriterScope(1)
            assertTrue(ledger.retainRuntimeWriter(main, "birth", warm))
            if (scenario != "unconfirmed") assertTrue(ledger.settleRuntimeWriter(main, "birth", warm, scenario != "no-acquisition-with-node"))
            val warmRow = runtimeStoreRun(warm, if (scenario == "wrong-path") "/different" else warm.stateDirectory, true)
            if (scenario == "wrong-digest") warmRow.put("configDigest", "9".repeat(64))
            val rows = mutableListOf(ScopedNativeFixture.retired(cold), warmRow)
            if (scenario == "foreign-empty") rows.add(runtimeStoreRun(runtimeWriterScope(2), null, true))
            custody.closed { ScopedNativeFixture.export(*rows.toTypedArray()) }
            assertTrue(scenario, custody.wire().getBoolean("historyUnknown"))
            assertEquals(scenario, null, custody.closedRelation(ScopedNativeFixture.file("old")))
        }
    }

    private fun runtimeStoreRun(scope: RuntimeTailscaleWriterScope, directory: String?, terminal: Boolean): JSONObject {
        val row = ScopedNativeFixture.run("0", "runtime", emptyList(), terminal)
            .put("runNonce", scope.runNonce).put("configDigest", scope.configDigest)
        if (directory != null) {
            if (terminal) row.put("terminal", "SealedDrained")
            val node = ScopedNativeFixture.node(scope.tag).put("stateDirectory", directory).put("stateFile", "$directory/tailscaled.state")
            if (terminal) node.put("writerState", "SealedDrained")
            row.getJSONArray("nodes").put(node)
        }
        return row
    }

    private fun runtimeWriterScope(sequence: Int): RuntimeTailscaleWriterScope {
        val directory = "/data/polaris/tailscale/node-$sequence"
        return RuntimeTailscaleWriterScope("incarnation", sequence.toLong(), 7, sequence.toString(16).padStart(64, '0'),
            "b".repeat(64), "ts-node-$sequence", directory, "$directory/Taildrop")
    }

    private fun AndroidNativeAdmission.retainRuntimeWriter(ticket: AndroidNativeAdmission.Ticket, birthNonce: String,
        scope: RuntimeTailscaleWriterScope): Boolean =
        retainRuntimeWriter(ticket, birthNonce, scope, RuntimeTailscaleStatePaths(scope.stateDirectory, scope.stateDirectory))

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
        // Simulate a lost release ACK: the original exact token remains retryable.
        assertTrue(ledger.tailscaleTargetAction(binding, file, "action-one", "finish").getBoolean("released"))
        assertTrue(runCatching { ledger.tailscaleTargetAction(binding, file, "second", "begin") }.isFailure)
    }

    @Test fun releaseReplayCannotReleaseSuccessorOrReuseActionIdentity() {
        val ledger = open()
        val (_, custody) = ScopedNativeFixture.closedOwner(ledger, "release-replay")
        val binding = custody.wire()
        val file = ScopedNativeFixture.file("one")
        ledger.tailscaleTargetAction(binding, file, "first", "begin")
        val released = ledger.tailscaleTargetAction(binding, file, "first", "finish")
        ledger.tailscaleTargetAction(binding, file, "second", "begin")
        assertEquals(released.toString(), ledger.tailscaleTargetAction(binding, file, "first", "finish").toString())
        assertTrue(runCatching { ledger.tailscaleTargetAction(binding, file, "third", "begin") }.isFailure)
        assertTrue(runCatching { ledger.tailscaleTargetAction(binding, file, "never-issued", "finish") }.isFailure)
        assertTrue(runCatching { ledger.tailscaleTargetAction(binding, ScopedNativeFixture.file("two"), "first", "finish") }.isFailure)
        ledger.tailscaleTargetAction(binding, file, "second", "finish")
        assertTrue(runCatching { ledger.tailscaleTargetAction(binding, file, "first", "begin") }.isFailure)
        assertEquals(2, ledger.metadataUsage().runtimeActions)
    }

    @Test fun releaseReceiptIsReservedBeforeGrantAndReplaySurvivesBudgetClosure() {
        val ledger = bounded(3)
        val (_, custody) = ScopedNativeFixture.closedOwner(ledger, "bounded-release")
        val binding = custody.wire()
        val file = ScopedNativeFixture.file("one")
        ledger.tailscaleTargetAction(binding, file, "last", "begin")
        assertEquals(3, ledger.metadataUsage().records)
        assertTrue(ledger.metadataUsage().capacityClosed)
        assertTrue(ledger.tailscaleTargetAction(binding, file, "last", "finish").getBoolean("released"))
        assertTrue(ledger.tailscaleTargetAction(binding, file, "last", "finish").getBoolean("released"))
        assertTrue(runCatching { ledger.tailscaleTargetAction(binding, file, "new", "begin") }.isFailure)
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
    fun cohort(parent: JSONObject, vararg children: JSONObject) = JSONObject()
        .put("contractVersion", "polaris-ts-startup-cohort-v1").put("censusComplete", true)
        .put("parent", parent).put("initialChildren", org.json.JSONArray(children.toList())).toString()
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
