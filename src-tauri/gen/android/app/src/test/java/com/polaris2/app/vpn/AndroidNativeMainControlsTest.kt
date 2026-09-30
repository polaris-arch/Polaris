package com.polaris2.app.vpn

import java.io.File
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.*
import org.junit.Test

/** Runs the same callback producer as BoxService with explicit fake JNI/Host boundaries. */
class AndroidNativeMainControlsTest {
    private fun await(latch: CountDownLatch) = check(latch.await(3, TimeUnit.SECONDS)) { "latch timeout" }
    private class Server {
        val reloads = AtomicInteger()
        val closes = AtomicInteger()
        val errors = CopyOnWriteArrayList<Throwable>()
    }
    private data class Target(val attempt: MainKernelAttempt<Server>, val server: Server)

    private inner class Harness(limit: Int = AndroidNativeAdmission.DEFAULT_MAX_METADATA_RECORDS) {
        val ledger = AndroidNativeAdmission("main-controls", maxMetadataRecords = limit).also {
            it.bootstrap(RequiredMarkerProof.Absent)
        }
        val controls = AndroidNativeMainControls(ledger)
        val registry = MainKernelAttemptLedger()
        val current = AtomicReference<Target?>()
        val lastOperation = AtomicReference<AndroidNativeAdmission.Ticket?>()
        val logged = CopyOnWriteArrayList<Throwable>()
        val events = CopyOnWriteArrayList<String>()
        val notices = AtomicInteger()
        var capture: () -> Unit = {}
        var load: () -> String = { "{}" }
        var preflight: (String) -> Unit = {}
        var hosts: (Target, () -> Unit) -> Unit = { target, native ->
            check(isCurrent(target))
            events.add("speedtest")
            events.add("login")
            native()
            check(isCurrent(target))
        }
        var native: () -> Unit = {}
        var report: () -> Unit = {}

        fun owner(id: String = "owner-a"): Target {
            val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, id)
            check(ledger.enterBirth(ticket))
            val attempt = MainKernelAttempt<Server>(runId = id, nativeTicket = ticket)
            val server = Server()
            attempt.publish(server)
            check(registry.claim(attempt))
            return Target(attempt, server).also { current.set(it) }
        }

        fun isCurrent(target: Target): Boolean = current.get() === target &&
            !target.attempt.revoked && registry.isCurrent(target.attempt)

        fun operation(): AndroidNativeAdmission.Entry = lastOperation.get()?.let {
            AndroidNativeAdmission.Entry(it, checkNotNull(ledger.state(it)))
        } ?: ledger.seal("fence").captured.single {
            it.ticket.kind == AndroidNativeAdmission.Kind.TargetlessReload
        }

        fun reload(target: Target) { lastOperation.set(controls.reload(
            target.attempt,
            target = {
                events.add("capture")
                capture()
                if (isCurrent(target)) target.server else null
            },
            allowed = { it === target.server && isCurrent(target) },
            loadConfig = { events.add("load"); load() },
            preflight = { events.add("preflight"); preflight(it) },
            withHosts = { server, _, action -> check(server === target.server); hosts(target, action) },
            nativeReload = { server, _ ->
                events.add("OverrideOptions")
                server.reloads.incrementAndGet()
                native()
            },
            setError = { server, failure -> server.errors.add(failure); report() },
            reconnectNotice = { notices.incrementAndGet() },
            logFailure = { logged.add(it) },
        )) }

        fun beginClose(target: Target): CountDownLatch {
            target.attempt.revokeAndDetachTun()
            val waiting = CountDownLatch(1)
            target.attempt.closeOnce { server ->
                waiting.countDown()
                synchronized(target.attempt.operationLock) { server?.closes?.incrementAndGet() }
            }
            return waiting
        }

        fun finishClose(target: Target) {
            assertNull(target.attempt.closed.get(3, TimeUnit.SECONDS))
            check(registry.completeAfterClose(target.attempt) { current.compareAndSet(target, null) })
            AndroidNativeMain.settleAfterExactRelease(ledger, target.attempt)
        }

        fun worker(target: Target): Pair<Thread, AtomicReference<Throwable?>> {
            val failure = AtomicReference<Throwable?>()
            val thread = Thread { try { reload(target) } catch (error: Throwable) { failure.set(error) } }
            thread.start()
            return Pair(thread, failure)
        }

        fun join(worker: Pair<Thread, AtomicReference<Throwable?>>) {
            worker.first.join(3000)
            assertFalse(worker.first.isAlive)
            assertNull(worker.second.get())
        }
    }

    @Test fun productionReservesBeforeCaptureAndConfigAndEntersBeforeOverrideOptions() {
        val h = Harness()
        val target = h.owner()
        h.capture = {
            val receipt = h.ledger.seal("fence")
            assertEquals(AndroidNativeAdmission.State.Reserved, receipt.captured.last().state)
            assertEquals(0, target.server.reloads.get())
        }
        h.reload(target)
        assertEquals(listOf("capture", "load", "preflight"), h.events)
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, h.operation().state)
        assertEquals(0, target.server.reloads.get())
        assertEquals(0, target.server.errors.size)
    }

    @Test fun normalReloadPreservesHostOrderAndCompletesOnlyControl() {
        val h = Harness()
        val target = h.owner()
        h.native = { assertEquals(AndroidNativeAdmission.State.BirthEntered, h.operation().state) }
        h.reload(target)
        assertEquals(listOf("capture", "load", "preflight", "speedtest", "login", "OverrideOptions"), h.events)
        assertEquals(1, target.server.reloads.get())
        assertEquals(AndroidNativeAdmission.State.Completed, h.operation().state)
        assertEquals(AndroidNativeAdmission.State.BirthEntered, h.ledger.state(target.attempt.nativeTicket!!))
        assertTrue(h.registry.isCurrent(target.attempt))
        assertFalse(target.attempt.constructionUnknown)
        assertTrue(h.logged.isEmpty())
    }

    @Test fun sealedIngressReservesNothingAndDoesNotResolveTargetOrLoadConfig() {
        val h = Harness()
        val target = h.owner()
        h.ledger.seal("fence")
        h.reload(target)
        assertTrue(h.events.isEmpty())
        assertEquals(1, h.ledger.metadataUsage().entries)
        assertTrue(h.logged.single() is AndroidNativeAdmission.AdmissionClosed)
        assertEquals(0, target.server.reloads.get())
        assertEquals(0, target.server.errors.size)
    }

    @Test fun sealDuringPurePreflightRefusesHostsAndAllNewJni() {
        val h = Harness()
        val target = h.owner()
        val entered = CountDownLatch(1)
        val resume = CountDownLatch(1)
        h.preflight = { entered.countDown(); await(resume) }
        val worker = h.worker(target)
        await(entered)
        assertEquals(AndroidNativeAdmission.State.Reserved, h.operation().state)
        resume.countDown()
        h.join(worker)
        assertFalse(h.events.contains("speedtest"))
        assertFalse(h.events.contains("OverrideOptions"))
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, h.operation().state)
        assertFalse(target.attempt.constructionUnknown)
    }

    @Test fun sealAfterHostPreflightStillBlocksActualOverrideAndConstruction() {
        val h = Harness()
        val target = h.owner()
        val entered = CountDownLatch(1)
        val resume = CountDownLatch(1)
        h.hosts = { _, action -> h.events.add("old-owner-cleanup"); entered.countDown(); await(resume); action() }
        val worker = h.worker(target)
        await(entered)
        assertEquals(AndroidNativeAdmission.State.Reserved, h.operation().state)
        resume.countDown()
        h.join(worker)
        assertFalse(h.events.contains("OverrideOptions"))
        assertEquals(0, target.server.reloads.get())
        assertEquals(0, target.server.errors.size)
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, h.operation().state)
        assertFalse(target.attempt.constructionUnknown)
    }

    @Test fun capacityRejectionDoesNotPreemptHostsAndKnownOwnerStillCloses() {
        val h = Harness(3)
        val target = h.owner()
        h.reload(target) // Last control reservation permanently closes admission.
        assertTrue(h.ledger.metadataUsage().capacityClosed)
        assertEquals(listOf("capture", "load", "preflight"), h.events)
        assertTrue(h.logged.single() is AndroidNativeAdmission.CapacityClosed)
        assertEquals(0, target.server.reloads.get())
        assertEquals(0, target.server.errors.size)
        await(h.beginClose(target))
        h.finishClose(target)
        assertEquals(1, target.server.closes.get())
        assertTrue(h.registry.isVacant())
        assertEquals(AndroidNativeAdmission.State.Unknown, h.ledger.state(target.attempt.nativeTicket!!))
    }

    @Test fun pureConfigAndHostErrorsPublishOnlyThroughTheSameTicket() {
        for (stage in listOf("load", "preflight", "host")) {
            val h = Harness()
            val target = h.owner()
            val first = IllegalArgumentException("safe $stage failure")
            when (stage) {
                "load" -> h.load = { throw first }
                "preflight" -> h.preflight = { throw first }
                else -> h.hosts = { _, _ -> throw first }
            }
            h.report = { assertEquals(AndroidNativeAdmission.State.BirthEntered, h.operation().state) }
            h.reload(target)
            assertSame(first, h.logged.single())
            assertSame(first, target.server.errors.single())
            assertEquals(0, target.server.reloads.get())
            assertEquals(AndroidNativeAdmission.State.Completed, h.operation().state)
            assertFalse(target.attempt.constructionUnknown)
            assertTrue(h.registry.isCurrent(target.attempt))
        }
    }

    @Test fun configErrorAfterSealRemainsFirstAndCannotPublishNativeError() {
        val h = Harness()
        val target = h.owner()
        val first = IllegalArgumentException("load rejected")
        h.load = { h.ledger.seal("fence"); throw first }
        h.reload(target)
        assertSame(first, h.logged.single())
        assertEquals(0, target.server.errors.size)
        assertTrue(first.suppressed.single() is AndroidNativeAdmission.AdmissionClosed)
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, h.operation().state)
        assertFalse(target.attempt.constructionUnknown)
    }

    @Test fun nativeFailureIsStickyBeforeErrorPublicationAndReportCannotReplaceFirstError() {
        val h = Harness()
        val target = h.owner()
        val first = IllegalStateException("partial native construction")
        val reporting = IllegalArgumentException("report failed")
        h.native = { throw first }
        h.report = {
            assertTrue(target.attempt.constructionUnknown)
            assertEquals(AndroidNativeAdmission.State.Unknown, h.operation().state)
            assertEquals(AndroidNativeAdmission.State.Unknown, h.ledger.state(target.attempt.nativeTicket!!))
            throw reporting
        }
        h.reload(target)
        assertSame(first, h.logged.single())
        assertSame(first, target.server.errors.single())
        assertSame(reporting, first.suppressed.single())
        await(h.beginClose(target))
        h.finishClose(target)
        assertEquals(AndroidNativeAdmission.State.Unknown, h.operation().state)
        assertEquals(AndroidNativeAdmission.State.Unknown, h.ledger.state(target.attempt.nativeTicket!!))
    }

    @Test fun errorReportingFailureBeforeConstructionPoisonsOnlyControl() {
        val h = Harness()
        val target = h.owner()
        val first = IllegalArgumentException("pure config error")
        val reporting = IllegalStateException("setError failed")
        h.load = { throw first }
        h.report = { throw reporting }
        h.reload(target)
        assertSame(first, h.logged.single())
        assertSame(reporting, first.suppressed.single())
        assertEquals(AndroidNativeAdmission.State.Unknown, h.operation().state)
        assertFalse(target.attempt.constructionUnknown)
        assertEquals(AndroidNativeAdmission.State.BirthEntered, h.ledger.state(target.attempt.nativeTicket!!))
    }

    @Test fun bothNativeFailureAndStopOrdersJoinOriginalOperationAndNeverReportToSuccessor() {
        for (stopFirst in listOf(false, true)) {
            val h = Harness()
            val target = h.owner()
            val entered = CountDownLatch(1)
            val nativeResume = CountDownLatch(1)
            val reporting = CountDownLatch(1)
            val reportResume = CountDownLatch(1)
            val first = IllegalStateException("native failed")
            h.native = { entered.countDown(); await(nativeResume); throw first }
            h.report = { reporting.countDown(); await(reportResume) }
            val worker = h.worker(target)
            await(entered)
            if (!stopFirst) { nativeResume.countDown(); await(reporting) }
            await(h.beginClose(target))
            try { target.attempt.closed.get(20, TimeUnit.MILLISECONDS); fail("Stop bypassed original work") }
            catch (_: TimeoutException) {}
            assertEquals(0, target.server.closes.get())
            if (stopFirst) nativeResume.countDown() else reportResume.countDown()
            h.join(worker)
            h.finishClose(target)
            val successor = h.owner("owner-b")
            assertEquals(1, target.server.closes.get())
            assertSame(first, h.logged.single())
            assertEquals(if (stopFirst) 0 else 1, target.server.errors.size)
            assertTrue(first.suppressed.any { it is AndroidNativeControlOperation.Revoked })
            assertEquals(0, successor.server.closes.get())
            assertEquals(0, successor.server.errors.size)
            assertEquals(AndroidNativeAdmission.State.Unknown, h.operation().state)
        }
    }

    @Test fun successfulJniThenRevokedHostCompletionRecordsBothUnknownBeforeClose() {
        val h = Harness()
        val target = h.owner()
        val hostFinish = CountDownLatch(1)
        val resume = CountDownLatch(1)
        h.hosts = { exact, action ->
            action()
            hostFinish.countDown()
            await(resume)
            check(h.isCurrent(exact)) { "Host completion revoked" }
        }
        val worker = h.worker(target)
        await(hostFinish)
        await(h.beginClose(target))
        assertFalse(target.attempt.closed.isDone)
        resume.countDown()
        h.join(worker)
        h.finishClose(target)
        assertTrue(target.attempt.constructionUnknown)
        assertEquals(1, target.server.reloads.get())
        assertEquals(0, target.server.errors.size)
        assertEquals(AndroidNativeAdmission.State.Unknown, h.operation().state)
        assertEquals(AndroidNativeAdmission.State.Unknown, h.ledger.state(target.attempt.nativeTicket!!))
    }

    @Test fun queuedOldCallbackAfterRealRegistryReleaseCannotSelectNewOwner() {
        val h = Harness()
        val original = h.owner()
        val captured = CountDownLatch(1)
        val resume = CountDownLatch(1)
        h.capture = { captured.countDown(); await(resume) }
        val worker = h.worker(original)
        await(captured)
        await(h.beginClose(original))
        h.finishClose(original)
        val successor = h.owner("owner-b")
        resume.countDown()
        h.join(worker)
        assertEquals(listOf("capture"), h.events)
        assertEquals(0, successor.server.reloads.get())
        assertEquals(0, successor.server.errors.size)
        assertEquals(0, successor.server.closes.get())
        assertTrue(h.registry.isCurrent(successor.attempt))
        assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, h.operation().state)
    }

    @Test fun dualModeDeclineKeepsCoreAndLateNoticeSkipsRevokedAttempt() {
        for (stop in listOf(false, true)) {
            val h = Harness()
            val target = h.owner()
            val preflight = CountDownLatch(1)
            val resume = CountDownLatch(1)
            h.preflight = { config ->
                preflight.countDown()
                await(resume)
                DualModeEndpointTombstone().requireReloadAllowed(true, config)
            }
            val worker = h.worker(target)
            await(preflight)
            if (stop) await(h.beginClose(target))
            resume.countDown()
            h.join(worker)
            assertEquals(if (stop) 0 else 1, h.notices.get())
            assertEquals(0, target.server.reloads.get())
            assertEquals(0, target.server.errors.size)
            assertFalse(target.attempt.constructionUnknown)
            assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, h.operation().state)
            if (stop) h.finishClose(target) else assertTrue(h.registry.isCurrent(target.attempt))
        }
    }

    @Test fun defaultUnboundStopAndReloadNeverLookUpCurrentEvenAfterFenceOrCapacity() {
        for (closure in listOf("open", "seal", "capacity")) {
            val h = Harness(if (closure == "capacity") 3 else AndroidNativeAdmission.DEFAULT_MAX_METADATA_RECORDS)
            val target = h.owner()
            if (closure == "seal") h.ledger.seal("fence")
            for (kind in listOf(AndroidNativeAdmission.Kind.TargetlessStop, AndroidNativeAdmission.Kind.TargetlessReload)) {
                val ticket = h.controls.rejectUnbound(kind) { h.logged.add(it) }
                if (ticket != null) assertEquals(AndroidNativeAdmission.State.CancelledBeforeBirth, h.ledger.state(ticket))
            }
            assertTrue(h.events.isEmpty())
            assertFalse(target.attempt.revoked)
            assertEquals(0, target.server.closes.get())
            assertEquals(0, target.server.reloads.get())
            assertEquals(0, target.server.errors.size)
            await(h.beginClose(target))
            h.finishClose(target)
            assertEquals(1, target.server.closes.get())
        }
    }

    @Test fun productionServiceAdaptersUseCapturedHandlersAndNoTerminalErrorJni() {
        val root = File("src/main/java/com/polaris2/app/vpn")
        val service = File(root, "BoxService.kt").readText()
        val producer = File(root, "AndroidNativeMainControls.kt").readText()
        val exact = service.substringAfter("private inner class AttemptHandler(").substringBefore("    override fun serviceStop() {\n        AndroidNativeMain.controls")
        assertTrue(exact.contains("stopService(attempt)"))
        assertTrue(exact.contains("this@BoxService.serviceReload(attempt)"))
        assertTrue(service.contains("Libbox.newStrictCommandServer(AttemptHandler(attempt, this), boundPlatform)"))
        val defaults = service.substringAfter("    override fun serviceStop() {\n        AndroidNativeMain.controls").substringBefore("private fun serviceReload(expectedAttempt:")
        assertFalse(defaults.contains("stopService("))
        assertFalse(defaults.contains("serviceReload(null)"))
        assertFalse(defaults.contains("mainAttempt"))
        assertTrue(defaults.contains("rejectUnbound(AndroidNativeAdmission.Kind.TargetlessReload)"))
        val reload = service.substringAfter("private fun serviceReload(expectedAttempt:").substringBefore("private fun setReloadError(")
        assertTrue(reload.contains("AndroidNativeMain.controls.reload("))
        assertTrue(reload.contains("TransientSpeedtestHost.withMainStart(attempt, { isReloadCurrent(attempt, server) })"))
        assertTrue(reload.contains("TransientLoginHost.withMainConfig(attempt, config, { isReloadCurrent(attempt, server) })"))
        assertFalse(reload.contains("AndroidNativeMain.construct("))
        val native = reload.substringAfter("nativeReload = { server, config ->").substringBefore("setError =")
        assertTrue(native.contains("server.startOrReloadService(config, OverrideOptions())"))
        assertTrue(native.contains("SystemStart.requireLegacyAllowed(service)"))
        assertTrue(producer.indexOf("reserve(ledger,") < producer.indexOf("target()"))
        assertTrue(producer.indexOf("target()") < producer.indexOf("loadConfig()"))
        assertTrue(producer.indexOf("boundary.requireBirthAllowed()") < producer.indexOf("withHosts(server, config)"))
        assertTrue(producer.contains("boundary.construct { nativeReload(server, config) }"))
        assertTrue(producer.contains("boundary.reportFailure(failure) { setError(server, failure) }"))
        val closed = service.substringAfter("private fun onAttemptClosed(").substringBefore("private fun registerStopReceiver(")
        assertFalse(closed.contains("setError("))
        assertTrue(closed.contains("AndroidNativeMain.unknown(attempt)"))
        assertTrue(closed.contains("VpnBridge.finishStop(\"android: 内核关闭失败\")"))
        assertEquals(setOf("main.bridge", "main.system", "validation.checkConfig"), AndroidNativeCoverage.wiredProducers)
    }
}
