package com.polaris2.app.vpn

import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MainNativeAdmissionWiringTest {
    private val plugin = File("src/main/java/com/polaris2/app/vpn/PolarisVpnPlugin.kt").readText()
    private val service = File("src/main/java/com/polaris2/app/vpn/BoxService.kt").readText()
    private val adapter = File("src/main/java/com/polaris2/app/vpn/AndroidNativeMain.kt").readText()

    @Test fun productionBridgeAndSystemBirthKeepTheSameTicketThroughTheService() {
        val bridgeStart = plugin.substringAfter("fun start(invoke: Invoke)").substringBefore("fun stop(invoke: Invoke)")
        val reserve = bridgeStart.indexOf("AndroidNativeMain.reserveBridge(args.runId)")
        val begin = bridgeStart.indexOf("VpnBridge.beginStart(MainStartRequest(")
        assertTrue(reserve >= 0 && reserve < begin)
        val serviceStart = service.substringAfter("fun onStartCommand()").substringBefore("fun onBind()")
        assertTrue(serviceStart.contains("request.nativeTicket"))
        assertTrue(serviceStart.contains("AndroidNativeMain.reserveSystem(systemRunId)"))
        assertTrue(serviceStart.contains("nativeTicket,"))
        val nativeStart = service.substringAfter("private fun startKernel(").substringBefore("// ── CommandServerHandler")
        val admission = nativeStart.indexOf("AndroidNativeMain.enterBirth(nativeTicket)")
        val setup = nativeStart.indexOf("PolarisApplication.ensureSetup()")
        val factory = nativeStart.indexOf("Libbox.newStrictCommandServer(")
        assertTrue(admission >= 0 && admission < setup && setup < factory)
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
}
