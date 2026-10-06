package com.polaris2.app.vpn

import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TransientLoginNativeOwnerTest {
    private fun ledger() = AndroidNativeAdmission("login-process").also { it.bootstrap(RequiredMarkerProof.Absent) }
    private fun reserve(ledger: AndroidNativeAdmission, id: String = "login-A") =
        TransientLoginNativeOwner.reserve(ledger, id)
    private fun await(latch: CountDownLatch) = assertTrue(latch.await(2, TimeUnit.SECONDS))

    @Test fun queuedOwnerAndValidationAreCapturedBeforeDispatchAndSealBlocksFirstResource() {
        val ledger = ledger()
        val owner = reserve(ledger)
        val queued = AtomicReference<(() -> Unit)?>()
        val failure = AtomicReference<Throwable?>()
        val acquisitions = AtomicInteger()
        owner.enqueue({ queued.set(it) }) { validation ->
            failure.set(runCatching { owner.construct { acquisitions.incrementAndGet() } }.exceptionOrNull())
            ledger.cancelBeforeBirth(validation)
            owner.closedWithoutProof()
        }
        val receipt = ledger.seal("login-fence")
        assertEquals(setOf(AndroidNativeAdmission.Kind.Login, AndroidNativeAdmission.Kind.CheckConfig),
            receipt.captured.map { it.ticket.kind }.toSet())
        assertTrue(receipt.captured.all { it.state == AndroidNativeAdmission.State.Reserved })
        requireNotNull(queued.get()).invoke()
        assertTrue(failure.get() is IllegalStateException)
        assertEquals(0, acquisitions.get())
        assertTrue(ledger.receipt("login-fence").captured.all { it.state == AndroidNativeAdmission.State.CancelledBeforeBirth })
        assertEquals(3, receipt.coveredProducers.size)
        assertFalse(receipt.coverageComplete)
    }

    @Test fun closeBeforeStartOvertakesDelayedParsingAndPermanentlyConsumesOriginalId() {
        val ledger = ledger()
        val parsing = CountDownLatch(1)
        val releaseParse = CountDownLatch(1)
        val done = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>()
        Thread {
            parsing.countDown()
            await(releaseParse)
            failure.set(runCatching { reserve(ledger) }.exceptionOrNull())
            done.countDown()
        }.start()
        await(parsing)
        TransientLoginNativeOwner.retireBeforeStart(ledger, "login-A")
        releaseParse.countDown()
        await(done)
        assertTrue(failure.get() is IllegalStateException)
        assertTrue(runCatching { reserve(ledger) }.isFailure)
        assertEquals(0, ledger.seal("login-fence").capturedCount)
    }

    @Test fun closeBeforeQueuedValidationCancelsOwnerAndLateWorkerCannotAcquireNative() {
        val ledger = ledger()
        val owner = reserve(ledger)
        val queued = AtomicReference<(() -> Unit)?>()
        var nativeCalls = 0
        owner.enqueue({ queued.set(it) }) { validation ->
            assertTrue(runCatching { owner.construct { nativeCalls++ } }.isFailure)
            ledger.cancelBeforeBirth(validation)
            owner.closedWithoutProof()
        }
        TransientLoginNativeOwner.retireBeforeStart(ledger, "login-A")
        assertTrue(owner.cancelled)
        requireNotNull(queued.get()).invoke()
        assertEquals(0, nativeCalls)
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(owner.ticket))
        assertTrue(runCatching { reserve(ledger) }.isFailure)
    }

    @Test fun failedDispatchCancelsBothReservationsAndCannotReplayAfterOperationalRemoval() {
        val ledger = ledger()
        val owner = reserve(ledger)
        val failure = runCatching {
            owner.enqueue({
                ledger.seal("login-fence")
                error("worker rejected")
            }) { error("must not execute") }
        }.exceptionOrNull()
        assertTrue(failure is IllegalStateException)
        assertTrue(owner.cancelled)
        assertTrue(ledger.receipt("login-fence").captured.all { it.state == AndroidNativeAdmission.State.CancelledBeforeBirth })

        val open = ledger()
        val removed = reserve(open)
        removed.closedWithoutProof()
        assertTrue(runCatching { reserve(open) }.isFailure)
        assertTrue(reserve(open, "login-B").enterBirth())
    }

    @Test fun nativeValidationFailureAndOldRetryExpiryOrStopCannotTouchSuccessorTicket() {
        val ledger = ledger()
        val old = reserve(ledger)
        val oldQueue = AtomicReference<(() -> Unit)?>()
        val failure = AtomicReference<Throwable?>()
        val callbacksQueued = CountDownLatch(1)
        val releaseCallbacks = CountDownLatch(1)
        val callbacksDone = CountDownLatch(1)
        val callbackFailure = AtomicReference<Throwable?>()
        // Queue the callbacks first; let the failed old worker and the new birth overtake them.
        Thread {
            callbackFailure.set(runCatching {
                val oldRetry = { old.cancel(); old.closedWithoutProof() }
                val oldExpiry = { old.cancel(); old.closedWithoutProof() }
                val oldServiceStop = { old.cancel(); old.closedWithoutProof() }
                callbacksQueued.countDown()
                await(releaseCallbacks)
                listOf(oldRetry, oldExpiry, oldServiceStop).forEach { it() }
            }.exceptionOrNull())
            callbacksDone.countDown()
        }.start()
        await(callbacksQueued)
        old.enqueue({ oldQueue.set(it) }) { validation ->
            failure.set(runCatching {
                old.construct {
                    AndroidNativeValidation.run(ledger, validation, setup = {}, nativeCheck = { error("checkConfig failed") })
                }
            }.exceptionOrNull())
            old.closedWithoutProof()
        }
        requireNotNull(oldQueue.get()).invoke()
        assertTrue(failure.get() is IllegalStateException)
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(old.ticket))
        val successor = reserve(ledger, "login-B")
        successor.construct { }
        releaseCallbacks.countDown()
        await(callbacksDone)
        assertEquals(null, callbackFailure.get())
        assertFalse(successor.cancelled)
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(successor.ticket))
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(old.ticket))
        assertEquals(AndroidNativeAdmission.State.ValidationCleanupUnknown,
            ledger.seal("login-fence").captured.single { it.ticket.kind == AndroidNativeAdmission.Kind.CheckConfig }.state)
    }

    @Test fun constructionFailureVersusCloseIsStickyUnknownInBothOrders() {
        for (closeFirst in listOf(false, true)) {
            val ledger = ledger()
            val owner = reserve(ledger)
            val entered = CountDownLatch(1)
            val release = CountDownLatch(1)
            val done = CountDownLatch(1)
            val failure = AtomicReference<Throwable?>()
            Thread {
                failure.set(runCatching {
                    owner.construct { entered.countDown(); await(release); error("partial box.New failure") }
                }.exceptionOrNull())
                done.countDown()
            }.start()
            await(entered)
            if (closeFirst) { owner.cancel(); owner.closedWithoutProof() }
            release.countDown()
            await(done)
            if (!closeFirst) { owner.cancel(); owner.closedWithoutProof() }
            assertTrue(failure.get() is IllegalStateException)
            assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(owner.ticket))
        }
    }

    @Test fun revokedLateSuccessfulConstructionCannotRestoreRunningOrProveRelease() {
        val ledger = ledger()
        val owner = reserve(ledger)
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val done = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>()
        Thread {
            failure.set(runCatching { owner.construct { entered.countDown(); await(release) } }.exceptionOrNull())
            done.countDown()
        }.start()
        await(entered)
        owner.cancel()
        release.countDown()
        await(done)
        assertTrue(failure.get() is IllegalStateException)
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(owner.ticket))
    }

    @Test fun mainPreemptionAndRepeatedOldDisposalStayWithOriginalOwner() {
        val ledger = ledger()
        val old = reserve(ledger)
        old.construct { }
        old.cancel()
        old.closedWithoutProof()
        val next = reserve(ledger, "login-B")
        next.construct { }
        TransientLoginNativeOwner.retireBeforeStart(ledger, "login-A")
        old.cancel()
        old.closedWithoutProof()
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(old.ticket))
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(next.ticket))
        assertFalse(next.cancelled)
        assertTrue(TransientLoginNativeOwner.capabilities.isEmpty())
    }

    @Test fun mainPreemptionBeforeQueuedOldWorkerCannotAffectTheLaterOwner() {
        val ledger = ledger()
        val old = reserve(ledger)
        val queued = CountDownLatch(1)
        val releaseWorker = CountDownLatch(1)
        val workerDone = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>()
        val acquisitions = AtomicInteger()
        old.enqueue({ action -> Thread {
            queued.countDown()
            await(releaseWorker)
            failure.set(runCatching { action() }.exceptionOrNull())
            workerDone.countDown()
        }.start() }) { validation ->
            try { old.construct { acquisitions.incrementAndGet() } }
            finally { ledger.cancelBeforeBirth(validation); old.closedWithoutProof() }
        }
        await(queued)
        old.cancel()
        old.closedWithoutProof()
        val next = reserve(ledger, "login-B")
        next.construct { }
        releaseWorker.countDown()
        await(workerDone)
        assertTrue(failure.get() is IllegalStateException)
        assertEquals(0, acquisitions.get())
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, ledger.state(old.ticket))
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(next.ticket))
        assertFalse(next.cancelled)
    }

    @Test fun consumedEpochIdsSurviveLongAttemptHistoryWithoutATtlOrEvictionWindow() {
        val ledger = ledger()
        val closedBeforeStart = "tailscale-login-node-0"
        TransientLoginNativeOwner.retireBeforeStart(ledger, closedBeforeStart)
        for (epoch in 1..2_050) reserve(ledger, "tailscale-login-node-$epoch").closedWithoutProof()
        for (old in listOf(closedBeforeStart, "tailscale-login-node-1", "tailscale-login-node-2050")) {
            assertTrue(runCatching { reserve(ledger, old) }.isFailure)
        }
        val next = reserve(ledger, "tailscale-login-node-2051")
        assertTrue(next.enterBirth())
        next.closedWithoutProof()
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(next.ticket))
        assertTrue(TransientLoginNativeOwner.capabilities.isEmpty())
    }

    @Test fun productionHostKeepsTicketsAndEveryDelayedCloseBoundToTheExactEntry() {
        val directory = File("src/main/java/com/polaris2/app/vpn")
        val host = File(directory, "TransientLoginHostState.kt").readText()
        val adapter = File(directory, "TransientLoginHost.kt").readText()
        val owner = File(directory, "TransientLoginNativeOwner.kt").readText()
        val start = host.substringAfter("fun start(id: String,").substringBefore("fun close(id:")
        assertTrue(start.indexOf("TransientLoginNativeOwner.reserve(") < start.indexOf("nativeOwner.enqueue("))
        assertTrue(start.indexOf("entry.nativeOwner.construct {") < start.indexOf("createEngine(id, config,"))
        assertTrue(owner.indexOf("check(enterBirth())") < owner.indexOf("val result = action()"))
        assertTrue(host.contains("schedule(300_000) { close(entry) {} }"))
        assertTrue(host.contains("schedule(5_000) { close(entry) {} }"))
        assertFalse(host.contains("close(id) {}"))
        assertTrue(adapter.contains("override fun serviceStop() { requestClose() }"))
        assertTrue(host.contains("createEngine(id, config, entry.stateDirectories) { close(entry) {} }"))
        val close = host.substringAfter("fun close(id: String,").substringBefore("fun running(")
        assertTrue(close.contains("TransientLoginNativeOwner.retireBeforeStart("))
        assertTrue(close.contains("if (entries[entry.id] === entry) entries.remove(entry.id)"))
        assertTrue(host.contains("entry.nativeOwner.closedWithoutProof()"))
        assertFalse(host.contains("closedExact("))
        assertFalse(adapter.contains("requireExactClose = true"))
    }

    @Test fun rustDropTimeoutAndConfirmedClosePreserveTheSameInstanceId() {
        val child = File("../../../src/runtime/tailscale_login_core.rs").readText()
        val bridge = File("../../../src/runtime/proxy/android_bridge.rs").readText()
        assertTrue(child.contains("start_transient_login(&child.instance_id, &config)"))
        assertTrue(child.contains("let id = self.instance_id.clone();"))
        assertTrue(child.contains("close_transient_login(&id).await"))
        assertTrue(child.contains("close_transient_login(&self.instance_id).await?"))
        val login = bridge.substringAfter("pub(crate) async fn start_transient_login(")
            .substringBefore("pub(crate) async fn transient_login_running(")
        assertTrue(login.contains("close_transient_login(instance_id).await"))
        assertEquals(2, login.split("instance_id: instance_id.to_owned()").size - 1)
    }

    @Test fun scopedStoreRetainsTheOriginalNativeOwnerAfterOrdinaryCloseWithoutChangingCoverage() {
        val ledger = ledger()
        val old = reserve(ledger)
        val store = old.bindTailscaleStore()
        assertTrue(store === old.bindTailscaleStore())
        var native = ScopedNativeFixture.export()
        val run = ScopedNativeFixture.run("a", "login", listOf("one"))
        old.construct { store.invoke("login", { native }) { native = ScopedNativeFixture.export(run) } }
        val binding = store.wire()
        store.closed { ScopedNativeFixture.export(ScopedNativeFixture.retired(run)) }
        old.closedWithoutProof()
        val next = reserve(ledger, "login-B")
        assertTrue(next.enterBirth())
        assertEquals(old.ticket.id, ledger.readTailscaleOwner(binding)!!.getString("nativeTicketId"))
        assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(old.ticket))
        assertEquals(AndroidNativeAdmission.State.BirthEntered, ledger.state(next.ticket))
        assertFalse(TransientLoginNativeOwner.capabilities.isNotEmpty())
        val foreign = org.json.JSONObject(binding.toString()).put("nativeTicketId", next.ticket.id)
        assertEquals(null, ledger.readTailscaleOwner(foreign))
    }

}
