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

    @Test fun mainAndValidationQueuedAdaptersRetainCapacityReasonWithoutCallingNative() {
        val ledger = AndroidNativeAdmission("capacity-adapters", maxMetadataRecords = 3).also { it.bootstrap(RequiredMarkerProof.Absent) }
        val main = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "queued-main")
        var worker: (() -> Unit)? = null
        AndroidNativeValidation.enqueue(ledger, { worker = it }) { ticket ->
            val failure = runCatching { AndroidNativeValidation.run(ledger, ticket, setup = { error("setup reached") }, nativeCheck = { error("JNI reached") }) }.exceptionOrNull()
            assertTrue(failure is AndroidNativeAdmission.CapacityClosed)
            assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(ticket))
        }
        val failure = runCatching { AndroidNativeMain.enterBirth(ledger, main) }.exceptionOrNull()
        assertTrue(failure is AndroidNativeAdmission.CapacityClosed)
        worker!!.invoke()
        ledger.cancelBeforeBirth(main)
        assertTrue(ledger.seal("fence").captured.isEmpty())
    }

    @Test fun allCurrentValidationEntrypointsUseTheSameAdapter() {
        val directory = File("src/main/java/com/polaris2/app/vpn")
        val plugin = File(directory, "PolarisVpnPlugin.kt").readText()
        val login = File(directory, "TransientLoginHost.kt").readText()
        val loginState = File(directory, "TransientLoginHostState.kt").readText()
        val speed = File(directory, "TransientSpeedtestHost.kt").readText()
        val adapter = File(directory, "AndroidNativeValidation.kt").readText()
        val command = plugin.substringAfter("fun checkConfig(invoke: Invoke)").substringBefore("fun vpnAuthStatus(")
        assertTrue(command.contains("AndroidNativeValidation.enqueue("))
        assertTrue(command.contains("AndroidNativeValidation.check(ticket, cfg)"))
        assertTrue(loginState.contains("nativeOwner.enqueue({ queue(it) })"))
        assertTrue(login.contains("queue = { worker.execute(it) }"))
        assertTrue(File(directory, "TransientLoginNativeOwner.kt").readText().contains("AndroidNativeValidation.enqueue(ledger, queue, action)"))
        assertTrue(login.contains("AndroidNativeValidation.check(validationTicket, value)"))
        assertTrue(speed.contains("AndroidNativeValidation.enqueue({ Thread(it,"))
        assertTrue(speed.contains("AndroidNativeValidation.check(validationTicket, config)"))
        assertFalse(adapter.contains("fun check(config: String)"))
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

    @Test fun allThreeIngressQueuesReserveBeforeDispatchAndRecheckBeforeNative() {
        for (ingress in listOf("plugin", "login", "speedtest")) {
            val ledger = AndroidNativeAdmission("process-1").also { it.bootstrap(RequiredMarkerProof.Absent) }
            val queued = CountDownLatch(1)
            val run = CountDownLatch(1)
            val done = CountDownLatch(1)
            val failure = AtomicReference<Throwable?>()
            var nativeCalls = 0
            AndroidNativeValidation.enqueue(ledger, queue = { action ->
                Thread({
                    queued.countDown()
                    await(run)
                    failure.set(runCatching { action() }.exceptionOrNull())
                    done.countDown()
                }, ingress).start()
            }) { ticket ->
                AndroidNativeValidation.run(ledger, ticket, setup = {}, nativeCheck = { nativeCalls++ })
            }
            await(queued)
            val receipt = ledger.seal("fence-1")
            assertEquals(AndroidNativeAdmission.State.Reserved, receipt.captured.single().state)
            run.countDown()
            await(done)
            assertTrue(failure.get() is AndroidNativeAdmission.AdmissionClosed)
            assertEquals(0, nativeCalls)
            assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth,
                ledger.receipt("fence-1").captured.single().state)
        }
    }

    @Test fun sealBeforeIngressNeverQueuesAndFailedDispatchCancelsReservation() {
        for (ingress in listOf("plugin", "login", "speedtest")) {
            val sealed = AndroidNativeAdmission("process-1").also { it.bootstrap(RequiredMarkerProof.Absent) }
            sealed.seal("fence-1")
            val rejection = runCatching {
                AndroidNativeValidation.enqueue(sealed, queue = { error("$ingress must not queue") }) { }
            }.exceptionOrNull()
            assertTrue(rejection is AndroidNativeAdmission.AdmissionClosed)
            assertEquals(0, sealed.receipt("fence-1").capturedCount)

            val ledger = AndroidNativeAdmission("process-2").also { it.bootstrap(RequiredMarkerProof.Absent) }
            var captured: AndroidNativeAdmission.Ticket? = null
            val unavailable = runCatching {
                AndroidNativeValidation.enqueue(ledger, queue = {
                    captured = ledger.seal("fence-2").captured.single().ticket
                    error("$ingress queue unavailable")
                }) { error("must not run") }
            }.exceptionOrNull()
            assertTrue(unavailable is IllegalStateException)
            assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(requireNotNull(captured)))
        }
    }

    @Test fun completeMembershipIsIndependentOfOuterCleanupAndNeverClosesRelatedWriters() {
        val request = "original-validation"
        val result = ScopedNativeFixture.validation(request, "config", listOf("one"))
        val scope = AndroidTailscaleValidationScope.capture(request, "config", result)
        assertEquals(false, scope.related(ScopedNativeFixture.file("other")))
        assertEquals(null, scope.related(ScopedNativeFixture.file("one")))
        val retired = ScopedNativeFixture.validation(request, "config", listOf("one"), writerTerminal = true)
        val sealed = AndroidTailscaleValidationScope.capture(request, "config", retired)
        assertEquals(true, sealed.related(ScopedNativeFixture.file("one")))
        assertEquals(false, sealed.related(ScopedNativeFixture.file("other")))
    }

    @Test fun missingGetterUnknownSchemaAndOriginalBindingMismatchCannotExcludeATarget() {
        val request = "original-validation"
        assertTrue(runCatching { AndroidTailscaleValidationScope.capture(request, "config", Any()) }.isFailure)
        for (mutation in listOf("request", "digest", "nonce", "schema", "targets", "unknown", "empty-export", "rejected")) {
            val result = ScopedNativeFixture.validation(request, "config", emptyList())
            val member = org.json.JSONObject(result.member)
            when (mutation) {
                "request" -> member.put("requestID", "successor")
                "digest" -> member.put("configDigest", "e".repeat(64))
                "nonce" -> member.put("runNonce", "e".repeat(64))
                "schema" -> member.put("extra", true)
                "targets" -> member.put("targets", org.json.JSONArray().put(ScopedNativeFixture.node("other")))
                "unknown" -> member.put("membershipState", "Unknown")
                "empty-export" -> result.retirement = ScopedNativeFixture.export()
                "rejected" -> result.validationState = "Rejected"
            }
            result.member = member.toString()
            val scope = runCatching { AndroidTailscaleValidationScope.capture(request, "config", result) }.getOrNull()
            assertTrue("$mutation was used as empty membership", scope == null || scope.related(ScopedNativeFixture.file("one")) == null)
        }
    }

    @Test fun actualRejectedNoConstructionExcludesOnlyThatValidationTicketAndCannotSelectWarm() {
        val request = "original-rejected"
        val result = ScopedNativeFixture.validation(request, "invalid-input", emptyList())
        result.validationState = "Rejected"; result.cleanupState = "NoConstruction"
        result.member = org.json.JSONObject(result.member).put("membershipState", "Unknown").toString()
        result.retirement = ScopedNativeFixture.export()
        val scope = AndroidTailscaleValidationScope.capture(request, "invalid-input", result)
        assertEquals(false, scope.related(ScopedNativeFixture.file("one")))
        assertFalse(scope.selectsWarm(ScopedNativeFixture.file("one")))
        for (mutation in listOf("accepted", "partial", "panic", "complete", "targets", "nonce", "request", "digest")) {
            val bad = ScopedNativeFixture.validation(request, "invalid-input", emptyList())
            bad.validationState = "Rejected"; bad.cleanupState = "NoConstruction"
            val member = org.json.JSONObject(bad.member).put("membershipState", "Unknown")
            when (mutation) {
                "accepted" -> bad.validationState = "Accepted"
                "partial" -> bad.cleanupState = "CleanupUnknown"
                "panic" -> bad.validationState = "InternalFailure"
                "complete" -> member.put("membershipState", "Complete")
                "targets" -> member.put("targets", org.json.JSONArray(listOf(org.json.JSONObject().put("tag", "one").put("stateDirectory", "/data/polaris/one").put("stateFile", ScopedNativeFixture.file("one")))))
                "nonce" -> member.put("runNonce", "not-a-nonce")
                "request" -> member.put("requestID", "other-original")
                "digest" -> member.put("configDigest", "f".repeat(64))
            }
            bad.member = member.toString(); bad.retirement = ScopedNativeFixture.export()
            assertEquals(mutation, null, runCatching { AndroidTailscaleValidationScope.capture(request, "invalid-input", bad).related(ScopedNativeFixture.file("one")) }.getOrNull())
        }
        assertTrue(runCatching { AndroidTailscaleValidationScope.capture("other", "invalid-input", result) }.isFailure)
        assertTrue(runCatching { AndroidTailscaleValidationScope.capture(request, "other", result) }.isFailure)
    }

    @Test fun warmChildIsRegisteredBeforeDispatchAndFailedQueueCancelsThatExactChild() {
        val ledger = AndroidNativeAdmission("warm-adapter").also { it.bootstrap(RequiredMarkerProof.Absent) }
        val tuple = ScopedNativeFixture.warm(ledger, "warm-child")
        ledger.beginWarm(tuple)
        val parent = ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, "warm-child")
        ledger.bindWarmConfig(parent, ScopedNativeFixture.warmConfig)
        var child: AndroidNativeAdmission.Ticket? = null
        assertTrue(runCatching { AndroidNativeValidation.enqueueWarm(ledger, parent, queue = {
            val management = ledger.readWarm(tuple)
            assertFalse(management.getBoolean("childRegistrationOpen"))
            assertTrue(management.getString("childNativeTicketId").isNotEmpty())
            child = ledger.seal("dispatch-fence").captured.single { it.ticket.id == management.getString("childNativeTicketId") }.ticket
            error("queue rejected before execution")
        }) { error("native work must not execute") } }.isFailure)
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(checkNotNull(child)))
        assertTrue(ledger.finishWarm(tuple).getBoolean("released"))
        assertFalse(ledger.enterBirth(parent)); assertFalse(ledger.enterBirth(checkNotNull(child)))
    }

}
