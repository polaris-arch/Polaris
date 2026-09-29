package com.polaris2.app.vpn

import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidNativeValidationTest {
    private fun await(latch: CountDownLatch) = assertTrue(latch.await(2, TimeUnit.SECONDS))

    @Test fun allCurrentValidationEntrypointsUseTheSameAdapter() {
        val directory = File("src/main/java/com/polaris2/app/vpn")
        val plugin = File(directory, "PolarisVpnPlugin.kt").readText()
        val login = File(directory, "TransientLoginHost.kt").readText()
        val speed = File(directory, "TransientSpeedtestHost.kt").readText()
        val adapter = File(directory, "AndroidNativeValidation.kt").readText()
        val command = plugin.substringAfter("fun checkConfig(invoke: Invoke)").substringBefore("fun vpnAuthStatus(")
        val reserve = command.indexOf("AndroidNativeValidation.reserve()")
        val queue = command.indexOf("val worker = Thread(")
        assertTrue(reserve >= 0 && reserve < queue)
        assertTrue(command.contains("AndroidNativeValidation.check(ticket, cfg)"))
        assertTrue(login.contains("AndroidNativeValidation.check(config)"))
        assertTrue(speed.contains("AndroidNativeValidation.check(config)"))
        assertFalse(plugin.contains("Libbox.checkConfig("))
        assertFalse(login.contains("Libbox.checkConfig("))
        assertFalse(speed.contains("Libbox.checkConfig("))
        assertEquals(1, adapter.split("Libbox.checkConfig(").size - 1)
    }

    @Test fun enteredValidationRemainsCapturedUntilJniActuallyReturns() {
        val ledger = AndroidNativeAdmission("process-1").also { it.bootstrap(RequiredMarkerProof.Absent) }
        val ticket = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>()
        val worker = Thread {
            failure.set(runCatching {
                AndroidNativeValidation.run(ledger, ticket, setup = {}, nativeCheck = {
                    entered.countDown()
                    await(release)
                })
            }.exceptionOrNull())
        }
        worker.start()
        await(entered)
        val sealed = ledger.seal("fence-1")
        assertEquals(AndroidNativeAdmission.State.BirthEntered, sealed.captured.single().state)
        assertFalse(ledger.receipt("fence-1").captured.single().state ==
            AndroidNativeAdmission.State.ValidationCleanupUnknown)
        release.countDown()
        worker.join(2_000)
        assertFalse(worker.isAlive)
        assertEquals(null, failure.get())
        assertEquals(AndroidNativeAdmission.State.ValidationCleanupUnknown,
            ledger.receipt("fence-1").captured.single().state)
    }

    @Test fun queuedValidationSealedBeforeWorkerCannotEnterJni() {
        val ledger = AndroidNativeAdmission("process-1").also { it.bootstrap(RequiredMarkerProof.Absent) }
        val ticket = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        val queued = CountDownLatch(1)
        val release = CountDownLatch(1)
        var invoked = false
        val failure = AtomicReference<Throwable?>()
        val worker = Thread {
            queued.countDown()
            await(release)
            failure.set(runCatching {
                AndroidNativeValidation.run(ledger, ticket, setup = {}, nativeCheck = { invoked = true })
            }.exceptionOrNull())
        }
        worker.start()
        await(queued)
        ledger.seal("fence-1")
        release.countDown()
        worker.join(2_000)
        assertFalse(worker.isAlive)
        assertTrue(failure.get() is AndroidNativeAdmission.AdmissionClosed)
        assertFalse(invoked)
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth,
            ledger.receipt("fence-1").captured.single().state)
    }
}
