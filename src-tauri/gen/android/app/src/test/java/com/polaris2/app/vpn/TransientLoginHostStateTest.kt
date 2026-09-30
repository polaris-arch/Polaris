package com.polaris2.app.vpn

import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.CancellationException
import java.util.concurrent.Executor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class TransientLoginHostStateTest {
    @get:Rule val temporary = TemporaryFolder()
    private fun await(latch: CountDownLatch) = assertTrue(latch.await(2, TimeUnit.SECONDS))

    private class Queue {
        private val tasks = ArrayDeque<() -> Unit>()
        var failure: Throwable? = null
        fun submit(action: () -> Unit) = synchronized(tasks) {
            failure?.let { throw it }
            tasks.addLast(action)
        }
        fun take(): () -> Unit = synchronized(tasks) { tasks.removeFirst() }
        fun runNext() = take().invoke()
        fun size(): Int = synchronized(tasks) { tasks.size }
    }

    private class Clock {
        private val tasks = mutableListOf<Pair<Long, () -> Unit>>()
        var failure: Throwable? = null
        fun schedule(delay: Long, action: () -> Unit) = synchronized(tasks) {
            failure?.let { throw it }
            tasks.add(delay to action)
        }
        fun take(delay: Long): () -> Unit = synchronized(tasks) {
            tasks.removeAt(tasks.indexOfFirst { it.first == delay }).second
        }
    }

    private inner class Fixture(limit: Int = AndroidNativeAdmission.DEFAULT_MAX_METADATA_RECORDS) {
        val ledger = AndroidNativeAdmission("host-process", maxMetadataRecords = limit).also { it.bootstrap(RequiredMarkerProof.Absent) }
        val queue = Queue()
        val cancellation = Queue()
        val clock = Clock()
        val directory = temporary.newFolder()
        val cache = File(directory, "login-cache-1.db")
        val stateFile = File(directory, "tailscale-state").apply { writeText("shared-state") }
        val engines = mutableMapOf<String, FakeEngine>()
        val failures = mutableListOf<String>()
        var factoryFailure: Throwable? = null
        var parseFailure: Throwable? = null
        var configure: (FakeEngine) -> Unit = { }
        val host = TransientLoginHostState(
            ledger = ledger,
            queue = queue::submit,
            schedule = clock::schedule,
            parseDirectories = { parseFailure?.let { throw it }; setOf(directory.canonicalPath) },
            requireSupported = { },
            createEngine = { id, _, directories, stop ->
                factoryFailure?.let { throw it }
                assertEquals(setOf(directory.canonicalPath), directories)
                FakeEngine(id, stop).also { configure(it); synchronized(engines) { engines[id] = it } }
            },
            logFailure = { stage, _, _ -> synchronized(failures) { failures.add(stage) } },
        )
        fun engine(id: String): FakeEngine = synchronized(engines) { requireNotNull(engines[id]) }
        fun ticket(id: String): AndroidNativeAdmission.Ticket = ledger.seal("host-fence").captured
            .single { it.ticket.kind == AndroidNativeAdmission.Kind.Login && it.ticket.logicalId == id }.ticket

        inner class FakeEngine(val id: String, val stop: () -> Unit) : TransientLoginHostState.Engine {
            val closes = AtomicInteger()
            val resolver = TransientResolverLifecycle(Executor { task -> cancellation.submit { task.run() } })
            val stages = mutableListOf<String>()
            var nativeRetained = true
            var networkRetained = true
            var fenceFailure: Throwable? = null
            var closeFailure: Throwable? = null
            var serverCloseFailure: Throwable? = null
            var networkCloseFailure: Throwable? = null
            var cacheFailure: Throwable? = null
            var prepareFailure: Throwable? = null
            var startAction: () -> Unit = { }
            var closeAction: () -> Unit = { }
            private val cleanup = TransientHostCleanup(
                beginResolverClose = { stages.add("fence"); resolver.beginClose(); fenceFailure?.let { throw it } },
                closeService = { stages.add("service"); closeAction(); closeFailure?.let { throw it } },
                closeServer = { stages.add("server"); serverCloseFailure?.let { throw it } },
                closeNetwork = { stages.add("network"); networkCloseFailure?.let { throw it } },
                nativeClosed = { nativeRetained = false },
                networkClosed = { networkRetained = false },
                closeCache = { stages.add("cache"); cacheFailure?.let { throw it }; check(!cache.exists() || cache.delete()) },
            )
            override fun prepare(validationTicket: AndroidNativeAdmission.Ticket, stage: (String) -> Unit, cancelled: () -> Boolean) {
                stage("check")
                AndroidNativeValidation.run(ledger, validationTicket, setup = {}, nativeCheck = {})
                cache.writeText(id)
                prepareFailure?.let { throw it }
                check(!cancelled())
            }
            override fun start() = startAction()
            override fun close() {
                closes.incrementAndGet()
                cleanup.close()
            }
        }
    }

    private class Reply<T> {
        val count = AtomicInteger()
        val value = AtomicReference<T>()
        val done = CountDownLatch(1)
        val callback: (T) -> Unit = { count.incrementAndGet(); value.set(it); done.countDown() }
    }

    @Test fun ownerReservationIsCancelledIfIndependentValidationCannotFit() {
        val f = Fixture(2)
        val reply = start(f, "login-A")
        await(reply.done)
        assertEquals(1, reply.count.get())
        assertEquals(AndroidNativeAdmission.CAPACITY_CODE, reply.value.get()!!.code)
        assertEquals(0, f.queue.size())
        assertTrue(f.engines.isEmpty())
        assertFalse(f.cache.exists())
        // The original owner cannot remain Reserved after failed enqueue.
        assertTrue(f.ledger.seal("host-fence").captured.isEmpty())
        assertEquals(AndroidNativeAdmission.MetadataUsage(1, 1, 2, true), f.ledger.metadataUsage())
    }

    @Test fun fullBudgetRejectsTheLastValidationAndEveryQueuedHostEntryBeforeFactory() {
        val f = Fixture(6)
        val first = start(f, "login-A")
        val last = start(f, "login-B")
        assertEquals(2, f.queue.size())
        assertTrue(f.ledger.metadataUsage().capacityClosed)
        f.queue.runNext()
        f.queue.runNext()
        for (reply in listOf(first, last)) {
            await(reply.done)
            assertEquals(1, reply.count.get())
            assertEquals(AndroidNativeAdmission.CAPACITY_CODE, reply.value.get()!!.code)
        }
        assertTrue(f.engines.isEmpty())
        assertFalse(f.cache.exists())
        assertTrue(f.ledger.seal("host-fence").captured.isEmpty())
    }

    @Test fun enteredHostStillClosesAtCapacityAndItsProofRemainsUnknown() {
        val f = Fixture(4)
        val started = start(f, "login-A")
        f.queue.runNext()
        assertEquals(null, started.value.get())
        f.ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "unseen")
        val closed = Reply<AndroidNativeFailure?>()
        f.host.closeCoded("login-A", closed.callback)
        f.queue.runNext()
        await(closed.done)
        assertEquals(null, closed.value.get())
        assertEquals(1, f.engine("login-A").closes.get())
        assertFalse(f.host.running("login-A"))
        assertFalse(f.cache.exists())
        val failed = Reply<AndroidNativeFailure?>()
        f.host.closeCoded("another-unseen", failed.callback)
        await(failed.done)
        assertEquals(AndroidNativeAdmission.CAPACITY_CODE, failed.value.get()!!.code)
        assertEquals(1, failed.count.get())
        val receipt = f.ledger.seal("host-fence")
        assertEquals(AndroidNativeAdmission.State.Unknown, receipt.captured.single { it.ticket.logicalId == "login-A" }.state)
        assertFalse(receipt.coverageComplete)
    }

    @Test fun capacityRejectedQueueCannotPreemptAnAlreadyEnteredSharedDirectoryOwner() {
        val f = Fixture(7)
        start(f, "login-A")
        f.queue.runNext()
        val queued = start(f, "login-B")
        f.ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "fill")
        f.queue.runNext()
        await(queued.done)
        assertEquals(AndroidNativeAdmission.CAPACITY_CODE, queued.value.get()!!.code)
        assertTrue(f.host.running("login-A"))
        assertEquals(0, f.engine("login-A").closes.get())
        assertEquals("login-A", f.cache.readText())
        val closed = close(f, "login-A")
        f.queue.runNext()
        await(closed.done)
        assertEquals(null, closed.value.get())
        assertFalse(f.cache.exists())
    }

    @Test fun actualCloseFailureIsNotReclassifiedByCapacityOrItsMessage() {
        val f = Fixture(4)
        start(f, "login-A")
        f.queue.runNext()
        f.ledger.retireOwner(AndroidNativeAdmission.Kind.Login, "unseen")
        f.engine("login-A").closeFailure = IllegalStateException(AndroidNativeAdmission.CAPACITY_MESSAGE)
        val closed = Reply<AndroidNativeFailure?>()
        f.host.closeCoded("login-A", closed.callback)
        f.queue.runNext()
        await(closed.done)
        assertEquals(null, closed.value.get()!!.code)
        assertTrue(f.host.running("login-A"))
        assertTrue(f.cache.exists())
        assertEquals(AndroidNativeAdmission.State.Unknown, f.ledger.state(f.ticket("login-A")))
    }

    private fun start(f: Fixture, id: String): Reply<TransientLoginHost.StartFailure?> = Reply<TransientLoginHost.StartFailure?>().also {
        f.host.start(id, "same-state-directory", it.callback)
    }
    private fun close(f: Fixture, id: String): Reply<String?> = Reply<String?>().also { f.host.close(id, it.callback) }

    @Test fun bothProductionHostsUseTheSameEarlyResolverFenceAndLoginKeepsOrdinaryNetworkClose() {
        val directory = File("src/main/java/com/polaris2/app/vpn")
        val login = File(directory, "TransientLoginHost.kt").readText()
        val speedtest = File(directory, "TransientSpeedtestHost.kt").readText()
        for (source in listOf(login, speedtest)) {
            assertTrue(source.contains("private val cleanup = TransientHostCleanup("))
            assertTrue(source.contains("beginResolverClose = { network?.beginResolverClose() }"))
            assertTrue(source.contains("cleanup.close()"))
        }
        assertTrue(login.contains("val createdNetwork = TransientLoginNetwork()"))
        assertTrue(speedtest.contains("resolverUnknown = { cleanupProof = false }"))
    }

    @Test fun loginFencesBeforeNativeJoinWithoutWaitingForSdkCancelAndReentrantOldCloseStaysIsolated() {
        val f = Fixture()
        start(f, "login-A")
        f.queue.runNext()
        val old = f.engine("login-A")
        val query = old.resolver.enterQuery<Int>()
        val queryDone = CountDownLatch(1)
        val writes = AtomicInteger()
        val error = AtomicReference<Throwable?>()
        val signalCalls = AtomicInteger()
        val ownershipLock = f.host.javaClass.getDeclaredField("ownershipLock").apply { isAccessible = true }.get(f.host)
        val resolverGate = old.resolver.javaClass.getDeclaredField("gate").apply { isAccessible = true }.get(old.resolver)
        query.installCancellation {
            assertFalse(Thread.holdsLock(ownershipLock))
            assertFalse(Thread.holdsLock(resolverGate))
            signalCalls.incrementAndGet()
        }
        Thread {
            try { query.awaitAndDeliver { writes.incrementAndGet() } }
            catch (_: CancellationException) { }
            catch (failure: Throwable) { error.set(failure) }
            finally { query.returned(); queryDone.countDown() }
        }.start()
        old.closeAction = {
            assertTrue(old.resolver.snapshot().sealed)
            assertTrue(runCatching { old.resolver.enterQuery<Int>() }.isFailure)
            await(queryDone) // Existing native Close joins query work; the earlier fence makes it return.
            old.stop() // Native callback reenters the exact Entry close while the outer close owns it.
        }
        val closed = close(f, "login-A")
        f.queue.runNext()
        await(closed.done)
        assertEquals(null, closed.value.get())
        assertEquals(null, error.get())
        assertEquals(listOf("fence", "service", "server", "network", "cache"), old.stages)
        assertEquals(0, writes.get())
        assertEquals(0, signalCalls.get())
        assertEquals(1, old.resolver.snapshot().cancellationTasks)
        assertEquals(1, f.cancellation.size())
        assertFalse(old.nativeRetained)
        assertFalse(old.networkRetained)
        assertFalse(f.cache.exists())
        f.queue.runNext()
        assertEquals(1, old.closes.get())
        val duplicate = close(f, "login-A")
        await(duplicate.done)
        start(f, "login-B")
        f.queue.runNext()
        val next = f.engine("login-B")
        old.stop()
        f.queue.runNext()
        f.cancellation.runNext()
        assertEquals(1, signalCalls.get())
        assertEquals(1, old.closes.get())
        assertEquals(0, next.closes.get())
        assertFalse(next.resolver.snapshot().sealed)
        assertEquals("login-B", f.cache.readText())
        assertTrue(f.host.running("login-B"))
        assertEquals(AndroidNativeAdmission.State.Unknown, f.ledger.state(f.ticket("login-A")))
    }

    @Test fun closeStagesKeepRealErrorPriorityAndOriginalReferenceAndCacheConditions() {
        for (failedStage in listOf("fence", "service", "server", "network", "cache")) {
            val f = Fixture()
            start(f, "login-A")
            f.queue.runNext()
            val engine = f.engine("login-A")
            val error = IllegalStateException("actual-$failedStage")
            when (failedStage) {
                "fence" -> engine.fenceFailure = error
                "service" -> {
                    engine.fenceFailure = IllegalStateException("secondary-fence")
                    engine.closeFailure = error
                    engine.serverCloseFailure = IllegalStateException("secondary-server")
                    engine.networkCloseFailure = TransientResolverLifecycle.CleanupUnknown()
                }
                "server" -> {
                    engine.serverCloseFailure = error
                    engine.networkCloseFailure = IllegalStateException("secondary-network")
                }
                "network" -> engine.networkCloseFailure = error
                "cache" -> engine.cacheFailure = error
            }
            val closed = close(f, "login-A")
            f.queue.runNext()
            await(closed.done)
            assertEquals(error.message, closed.value.get())
            val expected = listOf("fence", "service", "server", "network") + if (failedStage == "cache") listOf("cache") else emptyList()
            assertEquals(expected, engine.stages)
            assertEquals(failedStage in listOf("fence", "service", "server"), engine.nativeRetained)
            assertEquals(failedStage != "cache", engine.networkRetained)
            assertTrue(f.cache.exists())
            assertTrue(f.host.running("login-A"))
            assertEquals(AndroidNativeAdmission.State.Unknown, f.ledger.state(f.ticket("login-A")))
        }
    }

    @Test fun loginOperationalCloseDoesNotReturnAlreadyDeliveringJniOrQueuedCancelCounts() {
        val f = Fixture()
        start(f, "login-A")
        f.queue.runNext()
        val old = f.engine("login-A")
        val query = old.resolver.enterQuery<Int>()
        val inJni = CountDownLatch(1)
        val releaseJni = CountDownLatch(1)
        val queryDone = CountDownLatch(1)
        val writes = AtomicInteger()
        val error = AtomicReference<Throwable?>()
        query.installCancellation { }
        query.publish(1)
        Thread {
            try { query.awaitAndDeliver { inJni.countDown(); await(releaseJni); writes.incrementAndGet() } }
            catch (failure: Throwable) { error.set(failure) }
            finally { query.returned(); queryDone.countDown() }
        }.start()
        try {
            await(inJni)
            val closed = close(f, "login-A")
            f.queue.runNext()
            await(closed.done)
            assertEquals(null, closed.value.get())
            assertEquals(1, old.resolver.snapshot().calls)
            assertEquals(1, old.resolver.snapshot().deliveries)
            assertEquals(1, old.resolver.snapshot().cancellationTasks)
            assertEquals(TransientResolverLifecycle.Drain.Unknown, old.resolver.awaitLocalDrain(0, TimeUnit.SECONDS))
            assertEquals(1, old.resolver.snapshot().deliveries)
            start(f, "login-B")
            f.queue.runNext()
            assertTrue(f.host.running("login-B"))
            assertFalse(f.engine("login-B").resolver.snapshot().sealed)
            assertFalse(query.publish(2))
        } finally { releaseJni.countDown() }
        await(queryDone)
        assertEquals(null, error.get())
        assertEquals(1, writes.get())
        assertEquals(0, old.resolver.snapshot().calls)
        assertEquals(0, old.resolver.snapshot().deliveries)
        assertEquals(1, old.resolver.snapshot().cancellationTasks)
        f.cancellation.runNext()
        assertEquals(0, old.resolver.snapshot().cancellationTasks)
        assertEquals(AndroidNativeAdmission.State.Unknown, f.ledger.state(f.ticket("login-A")))
    }

    @Test fun queuedMainPreemptAndLateOldWorkerDoNotCreateNativeOrDeleteSuccessorCache() {
        val f = Fixture()
        val oldReply = start(f, "login-A")
        val delayedOldWorker = f.queue.take()
        val main = MainKernelAttempt<Any>()
        var mainStarts = 0
        f.host.withMainConfig(main, "same-state-directory") { mainStarts++ }
        f.host.closeMain(main) { }
        val nextReply = start(f, "login-B")
        f.queue.runNext()
        assertEquals(null, nextReply.value.get())
        assertEquals("login-B", f.cache.readText())
        delayedOldWorker()
        await(oldReply.done)
        assertTrue(oldReply.value.get() is TransientLoginHost.GeneralFailure)
        assertFalse(f.engines.containsKey("login-A"))
        assertTrue(f.host.running("login-B"))
        assertEquals("login-B", f.cache.readText())
        assertEquals("shared-state", f.stateFile.readText())
        assertEquals(1, mainStarts)
        assertEquals(1, oldReply.count.get())
        assertEquals(1, nextReply.count.get())
    }

    @Test fun stopWaitsForLateJniAndBothWorkerAndCloseCallbacksCompleteOnce() {
        val f = Fixture()
        val inJni = CountDownLatch(1)
        val releaseJni = CountDownLatch(1)
        f.configure = { it.startAction = { inJni.countDown(); await(releaseJni) } }
        val started = start(f, "login-A")
        val workerFinished = CountDownLatch(1)
        Thread { f.queue.runNext(); workerFinished.countDown() }.start()
        await(inJni)
        val stopped = close(f, "login-A")
        val closeStarted = CountDownLatch(1)
        Thread { closeStarted.countDown(); f.queue.runNext() }.start()
        await(closeStarted)
        assertFalse(stopped.done.await(30, TimeUnit.MILLISECONDS))
        assertEquals(0, f.engine("login-A").closes.get())
        releaseJni.countDown()
        await(started.done)
        await(stopped.done)
        await(workerFinished)
        assertTrue(started.value.get() is TransientLoginHost.GeneralFailure)
        assertEquals(null, stopped.value.get())
        assertFalse(f.host.running("login-A"))
        assertFalse(f.cache.exists())
        assertEquals(1, started.count.get())
        assertEquals(1, stopped.count.get())
        assertEquals(1, f.engine("login-A").closes.get())
        assertEquals(AndroidNativeAdmission.State.Unknown, f.ledger.state(f.ticket("login-A")))
    }

    @Test fun oldExpiryAndNativeStopKeepExactDisposedEntryAfterSameCacheSuccessorStarts() {
        val f = Fixture()
        start(f, "login-A")
        f.queue.runNext()
        val expiry = f.clock.take(300_000)
        val oldEngine = f.engine("login-A")
        val closed = close(f, "login-A")
        f.queue.runNext()
        await(closed.done)
        assertFalse(f.cache.exists())
        start(f, "login-B")
        f.queue.runNext()
        expiry()
        oldEngine.stop()
        f.queue.runNext()
        f.queue.runNext()
        assertEquals(1, oldEngine.closes.get())
        assertEquals(0, f.engine("login-B").closes.get())
        assertEquals("login-B", f.cache.readText())
        assertTrue(f.host.running("login-B"))
        assertEquals("shared-state", f.stateFile.readText())
    }

    @Test fun oldFailedCloseRetryCannotDeleteNewCacheAfterPredecessorDisposes() {
        val f = Fixture()
        start(f, "login-A")
        f.queue.runNext()
        val old = f.engine("login-A")
        old.closeFailure = IllegalStateException("network close failed")
        val firstClose = close(f, "login-A")
        f.queue.runNext()
        assertTrue(firstClose.value.get()!!.contains("network close failed"))
        assertEquals("login-A", f.cache.readText())
        val retry = f.clock.take(5_000)
        old.closeFailure = null
        val next = start(f, "login-B")
        f.queue.runNext()
        assertEquals(null, next.value.get())
        assertEquals(2, old.closes.get())
        assertEquals("login-B", f.cache.readText())
        retry()
        f.queue.runNext()
        assertEquals(2, old.closes.get())
        assertEquals(0, f.engine("login-B").closes.get())
        assertEquals("login-B", f.cache.readText())
        assertTrue(f.host.running("login-B"))
    }

    @Test fun actualCloseFailureBlocksSuccessorAndMainClaimUntilCleanupSucceeds() {
        val f = Fixture()
        start(f, "login-A")
        f.queue.runNext()
        f.engine("login-A").closeFailure = IllegalStateException("native close failed")
        val failed = close(f, "login-A")
        f.queue.runNext()
        assertTrue(failed.value.get()!!.contains("native close failed"))
        val next = start(f, "login-B")
        f.queue.runNext()
        assertTrue(next.value.get() is TransientLoginHost.GeneralFailure)
        assertFalse(f.engines.containsKey("login-B"))
        val main = MainKernelAttempt<Any>()
        val mainStarts = AtomicInteger()
        assertTrue(runCatching { f.host.withMainConfig(main, "same-state-directory") { mainStarts.incrementAndGet() } }.isFailure)
        assertEquals(0, mainStarts.get())
        assertEquals("login-A", f.cache.readText())
        assertEquals(1, failed.count.get())
        assertEquals(1, next.count.get())
    }

    @Test fun mainClaimIsVisibleWhileMainJniBlocksAndStopCannotRestoreOldClaim() {
        val f = Fixture()
        val main = MainKernelAttempt<Any>()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val finished = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>()
        Thread {
            failure.set(runCatching { f.host.withMainConfig(main, "same-state-directory", { !main.revoked }) {
                entered.countDown(); await(release)
            } }.exceptionOrNull())
            finished.countDown()
        }.start()
        await(entered)
        val login = start(f, "login-A")
        f.queue.runNext()
        assertTrue(login.value.get()!!.message.contains("ownership"))
        assertFalse(f.engines.containsKey("login-A"))
        main.revokeAndDetachTun()
        f.host.closeMain(main) { }
        release.countDown()
        await(finished)
        assertTrue(failure.get() is IllegalStateException)
        val next = start(f, "login-B")
        f.queue.runNext()
        assertEquals(null, next.value.get())
        assertTrue(f.host.running("login-B"))
    }

    @Test fun closeBeforeStartAndQueueFailuresReplyOnceWithoutNativeOrReplay() {
        val f = Fixture()
        val beforeStart = close(f, "login-A")
        await(beforeStart.done)
        val replay = start(f, "login-A")
        await(replay.done)
        assertTrue(replay.value.get() is TransientLoginHost.GeneralFailure)
        assertEquals(0, f.queue.size())
        f.queue.failure = IllegalStateException("rejected")
        val queued = start(f, "login-B")
        await(queued.done)
        assertEquals(1, queued.count.get())
        assertFalse(f.engines.containsKey("login-B"))
        f.queue.failure = null
        val secondReplay = start(f, "login-B")
        await(secondReplay.done)
        assertTrue(secondReplay.value.get() is TransientLoginHost.GeneralFailure)
        assertEquals(0, f.queue.size())
    }

    @Test fun factoryPrepareAndTimerExceptionsDisposeAndCompleteTheCallerOnce() {
        for (failureAt in listOf("factory", "prepare", "expiry")) {
            val f = Fixture()
            when (failureAt) {
                "factory" -> f.factoryFailure = IllegalStateException("partial factory")
                "prepare" -> f.configure = { it.prepareFailure = IllegalStateException("partial prepare") }
                "expiry" -> f.clock.failure = IllegalStateException("timer unavailable")
            }
            val result = start(f, "login-A")
            f.queue.runNext()
            await(result.done)
            assertEquals(1, result.count.get())
            assertTrue(result.value.get() is TransientLoginHost.GeneralFailure)
            assertFalse(f.host.running("login-A"))
            assertFalse(f.cache.exists())
            assertEquals(AndroidNativeAdmission.State.Unknown, f.ledger.state(f.ticket("login-A")))
        }
    }

    @Test fun callbackExceptionsDoNotBecomeQueueFailureOrRemoveARunningOwner() {
        val f = Fixture()
        val startReplies = AtomicInteger()
        f.host.start("login-A", "same-state-directory") { startReplies.incrementAndGet(); error("caller failed") }
        f.queue.runNext()
        assertEquals(1, startReplies.get())
        assertTrue(f.host.running("login-A"))
        assertEquals(0, f.engine("login-A").closes.get())
        assertEquals("login-A", f.cache.readText())
        val closeReplies = AtomicInteger()
        f.host.close("login-A") { closeReplies.incrementAndGet(); error("caller failed") }
        f.queue.runNext()
        assertEquals(1, closeReplies.get())
        assertEquals(1, f.engine("login-A").closes.get())
        assertFalse(f.host.running("login-A"))
        assertFalse(f.cache.exists())
        assertEquals(listOf("callback", "callback"), f.failures)
    }

    @Test fun failedCloseQueueAndRetryTimerPreserveOwnerAndReplyOnce() {
        val f = Fixture()
        start(f, "login-A")
        f.queue.runNext()
        f.queue.failure = IllegalStateException("close queue rejected")
        val rejected = close(f, "login-A")
        await(rejected.done)
        assertEquals(1, rejected.count.get())
        assertTrue(f.host.running("login-A"))
        assertEquals(0, f.engine("login-A").closes.get())
        f.queue.failure = null
        f.engine("login-A").closeFailure = IllegalStateException("close failed")
        f.clock.failure = IllegalStateException("retry timer rejected")
        val retryRejected = close(f, "login-A")
        f.queue.runNext()
        await(retryRejected.done)
        assertEquals(1, retryRejected.count.get())
        assertTrue(f.host.running("login-A"))
        assertEquals("login-A", f.cache.readText())
        assertEquals(listOf("retry"), f.failures)
    }
}
