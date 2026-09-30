package com.polaris2.app.vpn

import java.io.File
import java.util.concurrent.CountDownLatch
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
            var closeFailure: Throwable? = null
            var prepareFailure: Throwable? = null
            var startAction: () -> Unit = { }
            var closeAction: () -> Unit = { }
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
                closeAction()
                closeFailure?.let { throw it }
                check(!cache.exists() || cache.delete())
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
