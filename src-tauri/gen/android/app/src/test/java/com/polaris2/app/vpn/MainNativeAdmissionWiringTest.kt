package com.polaris2.app.vpn

import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MainNativeAdmissionWiringTest {
    private fun await(latch: CountDownLatch) = assertTrue(latch.await(2, TimeUnit.SECONDS))
    private val plugin = File("src/main/java/com/polaris2/app/vpn/PolarisVpnPlugin.kt").readText()
    private val service = File("src/main/java/com/polaris2/app/vpn/BoxService.kt").readText()
    private val adapter = File("src/main/java/com/polaris2/app/vpn/AndroidNativeMain.kt").readText()

    @Test fun productionBridgeAndSystemBirthKeepTheSameTicketThroughTheService() {
        val bridgeStart = plugin.substringAfter("fun start(invoke: Invoke)").substringBefore("fun stop(invoke: Invoke)")
        val reserve = bridgeStart.indexOf("AndroidNativeMain.reserveBridge(args.runId)")
        val begin = bridgeStart.indexOf("VpnBridge.beginStart(MainStartRequest(")
        assertTrue(reserve >= 0 && reserve < begin)
        val serviceStart = service.substringAfter("fun onStartCommand(startId: Int)").substringBefore("fun onBind()")
        assertTrue(serviceStart.contains("request.nativeTicket"))
        assertTrue(serviceStart.contains("AndroidNativeMain.reserveSystem(systemRunId)"))
        assertTrue(serviceStart.contains("nativeTicket,"))
        val nativeStart = service.substringAfter("private fun startKernel(").substringBefore("// ── CommandServerHandler")
        val admission = nativeStart.indexOf("AndroidNativeMain.enterBirth(nativeTicket)")
        val setup = nativeStart.indexOf("PolarisApplication.ensureSetup()")
        val factory = nativeStart.indexOf("Libbox.newStrictCommandServer(")
        assertTrue(admission >= 0 && admission < setup && setup < factory)
        val operation = nativeStart.indexOf("AndroidNativeMain.construct(attempt)")
        val ownership = nativeStart.indexOf("TransientLoginHost.withMainConfig(attempt,")
        assertTrue(operation >= 0 && operation < ownership && ownership < factory)
        val reload = service.substringAfter("private fun serviceReload(expectedAttempt:").substringBefore("private fun setReloadError(")
        assertTrue(reload.indexOf("AndroidNativeMain.construct(attempt)") <
            reload.indexOf("TransientLoginHost.withMainConfig(attempt,"))
        assertTrue(serviceStart.contains("LegacySystemStartFence.admitCurrentRequest(request, VpnBridge::currentStartRequest)"))
        assertTrue(serviceStart.contains("service.stopSelfResult(startId)"))
        assertFalse(serviceStart.contains("service.stopSelf()"))
        val close = service.substringAfter("private fun onAttemptClosed(").substringBefore("private fun registerStopReceiver(")
        val exactRelease = close.indexOf("MainKernelAttemptRegistry.completeAfterClose(attempt)")
        val settled = close.indexOf("AndroidNativeMain.settleAfterExactRelease(attempt)")
        assertTrue(exactRelease >= 0 && exactRelease < settled)
        assertTrue(adapter.contains("ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, runId)"))
        assertTrue(adapter.contains("ledger.enterBirth(ticket)"))
        assertTrue(adapter.contains("attempt.released.isDone"))
        assertTrue(adapter.contains("ledger.closedExact(ticket)"))
    }

    @Test fun sealDuringPendingToAttemptTransferCannotCreateNativeServer() {
        val ledger = AndroidNativeAdmission("process-1").also { it.bootstrap(RequiredMarkerProof.Absent) }
        val pendingTicket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "run-1")
        val pending = CountDownLatch(1)
        val resume = CountDownLatch(1)
        var entered = true
        val worker = Thread {
            pending.countDown()
            check(resume.await(2, TimeUnit.SECONDS))
            val attempt = MainKernelAttempt<Any>(runId = "run-1", nativeTicket = pendingTicket)
            entered = ledger.enterBirth(attempt.nativeTicket!!)
            if (!entered) ledger.cancelBeforeBirth(attempt.nativeTicket)
        }
        worker.start()
        assertTrue(pending.await(2, TimeUnit.SECONDS))
        val sealed = ledger.seal("fence-1")
        assertEquals(pendingTicket.id, sealed.captured.single().ticket.id)
        resume.countDown()
        worker.join(2_000)
        assertFalse(worker.isAlive)
        assertFalse(entered)
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth,
            ledger.receipt("fence-1").captured.single().state)
    }

    @Test fun enteredOwnerSettlesOnlyAfterExactRegistryRelease() {
        val ledger = AndroidNativeAdmission("process-1").also { it.bootstrap(RequiredMarkerProof.Absent) }
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "run-1")
        val attempt = MainKernelAttempt<String>(runId = "run-1", nativeTicket = ticket)
        val registry = MainKernelAttemptLedger()
        assertTrue(registry.claim(attempt))
        assertTrue(ledger.enterBirth(ticket))
        attempt.publish("native-server")
        attempt.closeOnce { assertEquals("native-server", it) }
        assertEquals(null, attempt.closed.get(2, TimeUnit.SECONDS))
        ledger.seal("fence-1")
        assertEquals(AndroidNativeAdmission.State.BirthEntered,
            ledger.receipt("fence-1").captured.single().state)
        assertTrue(registry.completeAfterClose(attempt) {})
        assertTrue(attempt.released.isDone)
        AndroidNativeMain.settleAfterExactRelease(ledger, attempt)
        assertEquals(AndroidNativeAdmission.State.ClosedExact,
            ledger.receipt("fence-1").captured.single().state)
    }

    @Test fun closeFailureCannotProduceExactReleaseReceipt() {
        val ledger = AndroidNativeAdmission("process-1").also { it.bootstrap(RequiredMarkerProof.Absent) }
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "run-1")
        val attempt = MainKernelAttempt<String>(runId = "run-1", nativeTicket = ticket)
        assertTrue(ledger.enterBirth(ticket))
        attempt.publish("native-server")
        attempt.closeOnce { throw IllegalStateException("close failed") }
        assertTrue(attempt.closed.get(2, TimeUnit.SECONDS) is IllegalStateException)
        ledger.seal("fence-1")
        AndroidNativeMain.settleAfterExactRelease(ledger, attempt)
        assertEquals(AndroidNativeAdmission.State.Unknown,
            ledger.receipt("fence-1").captured.single().state)
    }

    @Test fun startAndReloadFailureCannotLoseToConcurrentStop() {
        for (reload in listOf(false, true)) for (stopFirst in listOf(false, true)) {
            constructionVersusStop(reload, stopFirst, throws = true)
        }
    }

    @Test fun cancelledNativeCompletionCannotLoseToStop() {
        constructionVersusStop(reload = false, stopFirst = true, throws = false)
        constructionVersusStop(reload = true, stopFirst = true, throws = false)
    }

    private fun constructionVersusStop(reload: Boolean, stopFirst: Boolean, throws: Boolean) {
        val ledger = AndroidNativeAdmission("process-1").also { it.bootstrap(RequiredMarkerProof.Absent) }
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "run-1")
        val attempt = MainKernelAttempt<String>(runId = "run-1", nativeTicket = ticket)
        val registry = MainKernelAttemptLedger()
        assertTrue(registry.claim(attempt))
        assertTrue(ledger.enterBirth(ticket))
        attempt.publish("native-server") // CommandServer publishes before box.New; also true for Reload.
        if (reload) AndroidNativeMain.construct(ledger, attempt) { }
        val entered = CountDownLatch(1)
        val leave = CountDownLatch(1)
        val failed = CountDownLatch(1)
        val closeWaiting = CountDownLatch(1)
        val receiptDelivered = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>()
        val native = Thread {
            failure.set(runCatching {
                AndroidNativeMain.construct(ledger, attempt) {
                    entered.countDown()
                    await(leave)
                    if (throws) error("partial box.New failure")
                }
            }.exceptionOrNull())
            failed.countDown()
        }
        attempt.closed.whenComplete { error, exceptional ->
            if (error == null && exceptional == null) {
                assertTrue(registry.completeAfterClose(attempt) { })
            }
            AndroidNativeMain.settleAfterExactRelease(ledger, attempt)
            receiptDelivered.countDown()
        }
        fun stop() {
            attempt.revokeAndDetachTun()
            attempt.closeOnce {
                closeWaiting.countDown()
                synchronized(attempt.operationLock) { }
            }
        }
        native.start()
        await(entered)
        ledger.seal("fence-1")
        if (stopFirst) {
            stop()
            await(closeWaiting)
            assertFalse(attempt.closed.isDone)
            leave.countDown()
        } else {
            leave.countDown()
            await(failed)
            stop()
        }
        await(receiptDelivered)
        native.join(2_000)
        assertFalse(native.isAlive)
        assertTrue(failure.get() is IllegalStateException)
        assertTrue(attempt.constructionUnknown)
        assertTrue(attempt.closeSucceeded())
        assertEquals(AndroidNativeAdmission.State.Unknown,
            ledger.receipt("fence-1").captured.single().state)
    }

    @Test fun exceptionalOrCancelledFutureAlwaysStaysUnknownAndDeliversStopCallback() {
        for (future in listOf("prepared", "closed", "released")) for (cancel in listOf(false, true)) {
            val ledger = AndroidNativeAdmission("process-1").also { it.bootstrap(RequiredMarkerProof.Absent) }
            val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "run-1")
            val attempt = MainKernelAttempt<String>(runId = "run-1", nativeTicket = ticket)
            assertTrue(ledger.enterBirth(ticket))
            val delivered = CountDownLatch(1)
            val selected = when (future) {
                "prepared" -> attempt.prepared
                "closed" -> attempt.closed
                else -> attempt.released
            }
            attempt.closed.whenComplete { _, _ ->
                AndroidNativeMain.settleAfterExactRelease(ledger, attempt)
                delivered.countDown()
            }
            if (cancel) selected.cancel(false) else selected.completeExceptionally(IllegalStateException("failed"))
            if (future != "prepared") attempt.publish("server")
            if (future != "released") attempt.released.complete(Unit)
            if (future != "closed") attempt.closed.complete(null)
            await(delivered)
            AndroidNativeMain.settleAfterExactRelease(ledger, attempt)
            assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
            if (future == "closed") {
                val registry = MainKernelAttemptLedger()
                assertTrue(registry.claim(attempt))
                assertFalse(registry.completeAfterClose(attempt) { error("must not acknowledge") })
                assertEquals("Unknown", registry.exactStatus(MainKernelExactTarget(attempt.runId, attempt.birthNonce)).state)
            }
        }
    }

    @Test fun lateOldConstructionSettlementCannotTouchSuccessorTicket() {
        val ledger = AndroidNativeAdmission("process-1").also { it.bootstrap(RequiredMarkerProof.Absent) }
        val oldTicket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "run-a")
        val nextTicket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "run-b")
        val old = MainKernelAttempt<String>(runId = "run-a", nativeTicket = oldTicket)
        val next = MainKernelAttempt<String>(runId = "run-b", nativeTicket = nextTicket)
        val registry = MainKernelAttemptLedger()
        assertTrue(registry.claim(old))
        assertTrue(ledger.enterBirth(oldTicket))
        old.publish("a")
        runCatching { AndroidNativeMain.construct(ledger, old) { error("failed box.New") } }
        AndroidNativeMain.construct(ledger, old) { } // A later successful reload cannot erase uncertainty.
        old.closed.complete(null)
        assertTrue(registry.completeAfterClose(old) { })
        assertTrue(registry.claim(next))
        assertTrue(ledger.enterBirth(nextTicket))
        ledger.seal("fence-1")
        AndroidNativeMain.settleAfterExactRelease(ledger, old)
        assertFalse(registry.completeAfterClose(old) { error("must not release b") })
        assertTrue(registry.isCurrent(next))
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(oldTicket))
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(nextTicket))
    }

    @Test fun failedFactoryBeforeServerPublicationStaysUnknownAfterEmptyClose() {
        val ledger = AndroidNativeAdmission("process-1").also { it.bootstrap(RequiredMarkerProof.Absent) }
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "run-1")
        val attempt = MainKernelAttempt<String>(runId = "run-1", nativeTicket = ticket)
        val registry = MainKernelAttemptLedger()
        assertTrue(registry.claim(attempt))
        assertTrue(ledger.enterBirth(ticket))
        val factoryFailure = runCatching {
            AndroidNativeMain.construct(ledger, attempt) { error("factory failed before publication") }
        }.exceptionOrNull()
        assertTrue(factoryFailure is IllegalStateException)
        attempt.skipPreparation()
        attempt.closeOnce { assertEquals(null, it) }
        assertEquals(null, attempt.closed.get(2, TimeUnit.SECONDS))
        assertTrue(registry.completeAfterClose(attempt) { })
        AndroidNativeMain.settleAfterExactRelease(ledger, attempt)
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
    }
}
