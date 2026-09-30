package com.polaris2.app.vpn

import io.nekohasekai.libbox.ExchangeContext
import io.nekohasekai.libbox.LocalDNSTransport
import io.nekohasekai.libbox.PlatformInterface
import java.io.File
import java.lang.reflect.Proxy
import java.util.concurrent.CancellationException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.*
import org.junit.Test

class MainAttemptDnsTest {
    private fun await(latch: CountDownLatch) = assertTrue(latch.await(2, TimeUnit.SECONDS))
    private class Queue : Executor {
        private val tasks = LinkedBlockingQueue<Runnable>()
        override fun execute(command: Runnable) { tasks.add(command) }
        fun runNext() { checkNotNull(tasks.poll()).run() }
        val size get() = tasks.size
    }
    private class Resolver(cancellation: Executor? = null) {
        val lifecycle = if (cancellation == null) TransientResolverLifecycle() else TransientResolverLifecycle(cancellation)
        val unusedCloses = AtomicInteger()
        val transport = object : LocalDNSTransport {
            override fun raw() = true
            override fun exchange(ctx: ExchangeContext, message: ByteArray) { error("no real JNI in fake") }
            override fun lookup(ctx: ExchangeContext, network: String, domain: String) { error("no real JNI in fake") }
        }
        val resource = MainAttemptDns(transport, lifecycle::beginClose) {
            lifecycle.closeUnused()
            unusedCloses.incrementAndGet()
        }
        fun attempt(runId: String = "main") = MainKernelAttempt<String>(runId = runId, dns = resource)
    }
    private fun unusedDelegate(getters: AtomicInteger): PlatformInterface = Proxy.newProxyInstance(
        PlatformInterface::class.java.classLoader, arrayOf(PlatformInterface::class.java),
    ) { _, method, _ ->
        if (method.name == "localDNSTransport") getters.incrementAndGet()
        error("attempt DNS must never fall back to the singleton delegate")
    } as PlatformInterface

    private fun cancelled(query: TransientResolverLifecycle.Query<Int>) {
        try { query.awaitAndDeliver { fail("revoked result entered JNI") }; fail("expected cancellation") }
        catch (_: CancellationException) { }
    }

    @Test fun boundGetterAndReloadKeepAWhileSuccessorOwnsFreshB() {
        val a = Resolver()
        val b = Resolver()
        val singleton = TransientResolverLifecycle()
        val singletonQuery = singleton.enterQuery<Int>()
        val fallbackCalls = AtomicInteger()
        val old = a.attempt("a")
        val next = b.attempt("b")
        val oldPlatform = old.dns!!.bindPlatform(unusedDelegate(fallbackCalls))
        val nextPlatform = next.dns!!.bindPlatform(unusedDelegate(fallbackCalls))
        assertSame(a.transport, oldPlatform.localDNSTransport())
        assertSame(a.transport, oldPlatform.localDNSTransport()) // Existing reload keeps the binding.
        assertNotSame(oldPlatform.localDNSTransport(), nextPlatform.localDNSTransport())
        old.revokeAndDetachTun()
        assertSame(a.transport, oldPlatform.localDNSTransport()) // A late native getter stays with sealed A.
        assertTrue(runCatching { a.lifecycle.enterQuery<Int>() }.isFailure)
        val nextQuery = b.lifecycle.enterQuery<Int>()
        assertTrue(nextQuery.publish(7))
        nextQuery.awaitAndDeliver { assertEquals(7, it) }
        nextQuery.returned()
        assertTrue(singletonQuery.publish(8))
        singletonQuery.awaitAndDeliver { assertEquals(8, it) }
        singletonQuery.returned()
        assertFalse(singleton.snapshot().sealed)
        assertEquals(0, fallbackCalls.get())
        next.revokeAndDetachTun()
    }

    @Test fun stopBeforePublicationSealsBeforePreparedAndNativeWaits() {
        val resolver = Resolver()
        val attempt = resolver.attempt()
        val queue = AtomicReference<Thread?>()
        val nativeCalls = AtomicInteger()
        val stateLock = Any()
        synchronized(stateLock) {
            attempt.revokeAndDetachTun()
            assertTrue(attempt.revoked)
            assertTrue(resolver.lifecycle.snapshot().sealed)
            attempt.closeOnce {
                assertTrue(resolver.lifecycle.snapshot().sealed)
                nativeCalls.incrementAndGet()
                assertEquals("late server", it)
            }
            assertFalse(attempt.prepared.isDone)
            assertFalse(attempt.closed.isDone)
            assertTrue(runCatching { resolver.lifecycle.enterQuery<Int>() }.isFailure)
            // This is the actual outer BoxService lock boundary, not only the attempt monitor.
            val lockAvailable = CountDownLatch(1)
            queue.set(Thread { synchronized(stateLock) { lockAvailable.countDown() } }.also { it.start() })
            assertFalse(lockAvailable.await(10, TimeUnit.MILLISECONDS))
        }
        attempt.publish("late server")
        assertNull(attempt.closed.get(2, TimeUnit.SECONDS))
        queue.get()!!.join(2_000)
        assertEquals(1, nativeCalls.get())
        assertSame(resolver.transport, attempt.dns!!.transport)
    }

    @Test fun fenceReturnsUnderOuterStateLockWhileOperationAndJniStayBlocked() {
        val resolver = Resolver()
        val attempt = resolver.attempt()
        val query = resolver.lifecycle.enterQuery<Int>()
        val jniEntered = CountDownLatch(1)
        val jniRelease = CountDownLatch(1)
        val jniDone = CountDownLatch(1)
        val operationEntered = CountDownLatch(1)
        val operationRelease = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>()
        query.publish(1)
        val call = Thread {
            try { query.awaitAndDeliver { jniEntered.countDown(); await(jniRelease) } }
            catch (error: Throwable) { failure.set(error) }
            finally { query.returned(); jniDone.countDown() }
        }
        val operation = Thread { synchronized(attempt.operationLock) { operationEntered.countDown(); await(operationRelease) } }
        call.start(); operation.start()
        try {
            await(jniEntered); await(operationEntered)
            synchronized(Any()) { attempt.revokeAndDetachTun() }
            assertTrue(attempt.revoked)
            assertEquals(1, resolver.lifecycle.snapshot().calls)
            assertEquals(1, resolver.lifecycle.snapshot().deliveries)
            assertEquals(TransientResolverLifecycle.Drain.Unknown, resolver.lifecycle.awaitLocalDrain(0, TimeUnit.SECONDS))
            assertFalse(jniDone.await(10, TimeUnit.MILLISECONDS))
            assertFalse(query.publish(2))
        } finally { jniRelease.countDown(); operationRelease.countDown() }
        await(jniDone); operation.join(2_000)
        assertNull(failure.get())
        assertEquals(0, resolver.lifecycle.snapshot().calls)
        assertEquals(0, resolver.lifecycle.snapshot().deliveries)
    }

    @Test fun queuedCancellationAndSdkCleanupRemainOwnedByAWhileBIsActive() {
        for (throwCancel in listOf(false, true)) {
            val cancellation = Queue()
            val sdk = Queue()
            val a = Resolver(cancellation)
            val b = Resolver()
            val old = a.attempt("a")
            val next = b.attempt("b")
            val query = a.lifecycle.enterQuery<Int>()
            val nextQuery = b.lifecycle.enterQuery<Int>()
            val signalCalls = AtomicInteger()
            val cleanup = AtomicInteger()
            query.installCancellation { signalCalls.incrementAndGet(); if (throwCancel) error("SDK cancel failed") }
            assertTrue(query.enterSdkSubmission())
            val executor = a.lifecycle.sdkExecutor(sdk)
            executor.execute { assertFalse(query.publish(1)); cleanup.incrementAndGet() }
            synchronized(Any()) { old.revokeAndDetachTun(); old.revokeAndDetachTun() }
            cancelled(query); query.returned()
            assertEquals(0, signalCalls.get()) // Enqueue does not run SDK cancellation inline under BoxService.
            assertEquals(1, a.lifecycle.snapshot().cancellationTasks)
            assertEquals(1, a.lifecycle.snapshot().sdkTasks)
            assertEquals(0, b.lifecycle.snapshot().cancellationTasks)
            cancellation.runNext()
            sdk.runNext()
            executor.execute { assertFalse(query.publish(2)); cleanup.incrementAndGet() }
            sdk.runNext() // A's late framework fd task is accepted even after local drain.
            assertEquals(2, cleanup.get())
            assertEquals(1, signalCalls.get())
            assertEquals(throwCancel, a.lifecycle.snapshot().cancellationUnknown)
            assertEquals(TransientResolverLifecycle.Drain.Unknown, a.lifecycle.awaitLocalDrain(0, TimeUnit.SECONDS))
            assertTrue(nextQuery.publish(3))
            nextQuery.awaitAndDeliver { assertEquals(3, it) }; nextQuery.returned()
            assertFalse(b.lifecycle.snapshot().sealed)
            next.revokeAndDetachTun()
        }
    }

    @Test fun blockingLegacyLookupCannotDisappearAtStopOrTimeout() {
        val resolver = Resolver()
        val attempt = resolver.attempt()
        val query = resolver.lifecycle.enterQuery<Int>()
        val lookupEntered = CountDownLatch(1)
        val lookupRelease = CountDownLatch(1)
        val returned = CountDownLatch(1)
        val call = Thread {
            try {
                assertTrue(query.canStartBlockingLookup())
                lookupEntered.countDown(); await(lookupRelease)
                assertFalse(query.publish(1)); cancelled(query)
            } finally { query.returned(); returned.countDown() }
        }
        call.start()
        try {
            await(lookupEntered)
            attempt.revokeAndDetachTun()
            assertEquals(1, resolver.lifecycle.snapshot().calls)
            assertEquals(TransientResolverLifecycle.Drain.Unknown, resolver.lifecycle.awaitLocalDrain(1, TimeUnit.MILLISECONDS))
            assertEquals(1, resolver.lifecycle.snapshot().calls)
            assertFalse(returned.await(10, TimeUnit.MILLISECONDS))
        } finally { lookupRelease.countDown() }
        await(returned)
        assertEquals(0, resolver.lifecycle.snapshot().calls)
    }

    @Test fun onlyNeverExposedResolverUsesUnusedDisposalAndNeverMainExact() {
        val unused = Resolver()
        val attempt = unused.attempt()
        attempt.revokeAndDetachTun()
        attempt.dns!!.closeUnused()
        assertEquals(1, unused.unusedCloses.get())
        assertTrue(unused.lifecycle.snapshot().sealed)
        assertFalse(unused.lifecycle.snapshot().queried)
        val queried = Resolver()
        val query = queried.lifecycle.enterQuery<Int>()
        query.cancel(); query.returned()
        assertTrue(runCatching { queried.resource.closeUnused() }.exceptionOrNull() is TransientResolverLifecycle.CleanupUnknown)
        assertEquals(0, queried.unusedCloses.get())
        assertFalse(AndroidNativeMain.capabilities.contains(AndroidNativeProducer.MainClose))
    }

    @Test fun productionServicePublishesTheOwnedResourceAndUsesItsBoundGetter() {
        val root = File("src/main/java/com/polaris2/app/vpn")
        val service = File(root, "BoxService.kt").readText()
        val start = service.substringAfter("fun onStartCommand(startId: Int)").substringBefore("fun onBind()")
        val create = start.indexOf("val dns = MainAttemptDns.create()")
        val construct = start.indexOf("val next = MainKernelAttempt<CommandServer>(")
        val claim = start.indexOf("MainKernelAttemptRegistry.claim(next)")
        val visible = start.indexOf("mainAttempt = next")
        assertTrue(create >= 0 && create < construct && construct < claim && claim < visible)
        assertTrue(start.contains("nativeTicket,\n                            dns,"))
        assertTrue(start.contains("if (!claimed) runCatching { dns.closeUnused() }"))
        val native = service.substringAfter("private fun startKernel(").substringBefore("// ── CommandServerHandler")
        val binding = native.indexOf("checkNotNull(attempt.dns).bindPlatform(platformInterface)")
        val factory = native.indexOf("Libbox.newStrictCommandServer(")
        assertTrue(binding >= 0 && binding < factory)
        assertTrue(native.contains("object : PlatformInterface by dnsPlatform"))
        val stop = service.substringAfter("private fun stopService(").substringBefore("private fun onAttemptClosed(")
        val revoke = stop.indexOf("attempt.revokeAndDetachTun()")
        val close = stop.indexOf("attempt.closeOnce")
        val monitor = stop.indexOf("DefaultNetworkMonitor.stop()")
        assertTrue(revoke >= 0 && revoke < close && revoke < monitor)
        assertFalse(stop.contains("closeUnused()"))
        val resource = File(root, "MainAttemptDns.kt").readText()
        assertTrue(resource.contains("override fun localDNSTransport(): LocalDNSTransport = transport"))
        assertFalse(Regex("\\bLocalResolver\\b").containsMatchIn(resource))
    }

    @Test fun actualFactoryCreatesFreshUnusedResolversWithoutSdkQueries() {
        val a = MainAttemptDns.create()
        val b = MainAttemptDns.create()
        assertTrue(a.transport is NetworkLocalResolver)
        assertTrue(b.transport is NetworkLocalResolver)
        assertNotSame(a.transport, b.transport)
        a.beginClose(); a.closeUnused()
        b.beginClose(); b.closeUnused()
        assertNull(a.fenceFailure)
        assertNull(b.fenceFailure)
    }

    @Test fun cancelledCloseQueueStillHasSealedDnsAndRetainsActualFailure() {
        val resolver = Resolver()
        val attempt = resolver.attempt()
        val nativeCloses = AtomicInteger()
        attempt.revokeAndDetachTun()
        attempt.prepared.cancel(false)
        attempt.closeOnce { nativeCloses.incrementAndGet() }
        assertTrue(attempt.closed.get(2, TimeUnit.SECONDS) is CancellationException)
        assertTrue(resolver.lifecycle.snapshot().sealed)
        assertTrue(runCatching { resolver.lifecycle.enterQuery<Int>() }.isFailure)
        assertEquals(0, nativeCloses.get())
    }

    @Test fun failingFenceDoesNotStrandNativeCloseOrHideItsResult() {
        for (failNative in listOf(false, true)) {
            val ledger = AndroidNativeAdmission("process").also { it.bootstrap(RequiredMarkerProof.Absent) }
            val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "a")
            assertTrue(ledger.enterBirth(ticket))
            val resolver = Resolver()
            val fenceError = IllegalStateException("fence failed")
            val resource = MainAttemptDns(resolver.transport, { throw fenceError }, resolver.lifecycle::closeUnused)
            val attempt = MainKernelAttempt<String>(runId = "a", nativeTicket = ticket, dns = resource)
            val registry = MainKernelAttemptLedger()
            assertTrue(registry.claim(attempt))
            attempt.publish("server")
            val nativeCloses = AtomicInteger()
            synchronized(Any()) { attempt.revokeAndDetachTun() }
            assertTrue(attempt.revoked)
            assertSame(fenceError, resource.fenceFailure)
            attempt.closeOnce { nativeCloses.incrementAndGet(); if (failNative) error("real native failure") }
            val nativeFailure = attempt.closed.get(2, TimeUnit.SECONDS)
            assertEquals(1, nativeCloses.get())
            assertEquals(failNative, nativeFailure != null)
            if (failNative) assertEquals("real native failure", nativeFailure!!.message)
            assertEquals(!failNative, registry.completeAfterClose(attempt) {})
            AndroidNativeMain.settleAfterExactRelease(ledger, attempt)
            assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
            assertSame(fenceError, resource.fenceFailure)
            assertFalse(AndroidNativeMain.capabilities.contains(AndroidNativeProducer.MainClose))
            resolver.lifecycle.beginClose()
        }
    }

    @Test fun productionCancellationExecutorNeverRunsBlockingSdkInlineUnderStateLock() {
        val resolver = Resolver() // The actual default lifecycle cancellation executor.
        val attempt = resolver.attempt()
        val query = resolver.lifecycle.enterQuery<Int>()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val finished = CountDownLatch(1)
        query.installCancellation { entered.countDown(); await(release); finished.countDown() }
        attempt.publish("server")
        try {
            synchronized(Any()) { attempt.revokeAndDetachTun() }
            await(entered)
            cancelled(query); query.returned()
            assertEquals(1, resolver.lifecycle.snapshot().cancellationTasks)
            attempt.closeOnce { assertEquals("server", it) }
            assertNull(attempt.closed.get(2, TimeUnit.SECONDS))
            assertEquals(1, resolver.lifecycle.snapshot().cancellationTasks)
        } finally { release.countDown() }
        await(finished)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, resolver.lifecycle.awaitLocalDrain(2, TimeUnit.SECONDS))
        assertEquals(0, resolver.lifecycle.snapshot().cancellationTasks)
    }

    @Test fun rejectedCancellationEnqueueRemainsUnknownWithoutBlockingNativeClose() {
        val resolver = Resolver(Executor { throw java.util.concurrent.RejectedExecutionException("rejected") })
        val attempt = resolver.attempt()
        val query = resolver.lifecycle.enterQuery<Int>()
        query.installCancellation { fail("rejected task executed") }
        attempt.publish("server")
        synchronized(Any()) { attempt.revokeAndDetachTun() }
        cancelled(query); query.returned()
        attempt.closeOnce { assertEquals("server", it) }
        assertNull(attempt.closed.get(2, TimeUnit.SECONDS))
        assertTrue(resolver.lifecycle.snapshot().cancellationUnknown)
        assertEquals(1, resolver.lifecycle.snapshot().cancellationTasks)
        assertEquals(TransientResolverLifecycle.Drain.Unknown, resolver.lifecycle.awaitLocalDrain(0, TimeUnit.SECONDS))
    }

    @Test fun operationalCloseFailureStaysFailureAndSuccessAllowsReconnectWithUnknownLedger() {
        for (failClose in listOf(false, true)) {
            val ledger = AndroidNativeAdmission("process").also { it.bootstrap(RequiredMarkerProof.Absent) }
            val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "a")
            val resolver = Resolver()
            val attempt = MainKernelAttempt<String>(runId = "a", nativeTicket = ticket, dns = resolver.resource)
            val registry = MainKernelAttemptLedger()
            assertTrue(registry.claim(attempt)); assertTrue(ledger.enterBirth(ticket))
            attempt.publish("server")
            attempt.revokeAndDetachTun()
            attempt.closeOnce { if (failClose) error("real native close failure") }
            val failure = attempt.closed.get(2, TimeUnit.SECONDS)
            assertEquals(failClose, failure != null)
            if (failClose) assertEquals("real native close failure", failure!!.message)
            assertEquals(!failClose, registry.completeAfterClose(attempt) {})
            AndroidNativeMain.settleAfterExactRelease(ledger, attempt)
            assertEquals(AndroidNativeAdmission.State.Unknown, ledger.state(ticket))
            val next = Resolver().attempt("b")
            assertEquals(!failClose, registry.claim(next))
            assertTrue(resolver.lifecycle.snapshot().sealed)
        }
    }
}
