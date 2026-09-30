package com.polaris2.app.vpn

import java.io.Closeable
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.Assert.*
import org.junit.Test

class DebugBatchQaTest {
    private val secret = ByteArray(32) { it.toByte() }
    private fun frame(role: String = "data", sequence: Long = 1) = DebugBatchProtocol.Frame(
        "REQ", "a".repeat(64), "b".repeat(32), "c".repeat(48), "d".repeat(32), role, "udp", sequence, "e".repeat(32), 5000)
    private fun counter() = DebugBatchCounter("a".repeat(64), "b".repeat(32), "c".repeat(48), "d".repeat(32), "udp", 5000)

    @Test fun wireRejectsWrongKeyRoleOversizeAndRequestEchoAsAck() {
        val original = frame()
        val wire = DebugBatchProtocol.encode(secret, original)
        assertEquals(original, DebugBatchProtocol.decode(secret, wire))
        assertEquals("c63f3fefd5d7777831464c1c0a9a8afd1ad8255aa1eb1932d4b9ede205e2470e", wire.toString(Charsets.US_ASCII).substringAfterLast('|'))
        assertNull(DebugBatchProtocol.decode(ByteArray(32), wire))
        assertNull(DebugBatchProtocol.decode(secret, wire + ByteArray(513)))
        assertNull(DebugBatchProtocol.decode(secret, wire.toString(Charsets.US_ASCII).replace("|data|", "|health|").toByteArray()))
        assertNotEquals(wire.toList(), DebugBatchProtocol.encode(secret, original.copy(kind = "ACK")).toList())
    }
    @Test fun healthDoesNotIncreaseDataMatchedAndSealRejectsLateFrames() {
        val c = counter()
        assertTrue(c.accept(frame("health"), 200, 1000)); assertEquals(0, c.snapshot().matched)
        assertTrue(c.accept(frame(), 200, 1000)); assertEquals(1, c.snapshot().matched)
        assertFalse(c.accept(frame(), 200, 1000)); assertEquals(1, c.snapshot().replay)
        assertFalse(c.accept(frame(sequence = 2).copy(nonce = "f".repeat(48)), 200, 1000))
        assertFalse(c.accept(frame(sequence = 2), 200, 5000))
        c.seal(); val frozen = c.snapshot()
        assertFalse(c.accept(frame(sequence = 2), 200, 1000)); assertEquals(frozen, c.snapshot())
    }
    @Test fun planRejectsDnsPublicPeerLeadingZeroPortAndLongTtl() {
        DebugBatchProtocol.validatePlan(listOf("192.168.10.1"), 47100, 300000)
        for (ip in listOf("localhost", "8.8.8.8", "192.168.010.1", "127.0.0.1", "192.168.1.255"))
            assertFalse(DebugBatchProtocol.literalLanIPv4(ip))
        assertTrue(runCatching { DebugBatchProtocol.validatePlan(listOf("10.0.0.1"), 80, 1000) }.isFailure)
        assertTrue(runCatching { DebugBatchProtocol.validatePlan(listOf("10.0.0.1"), 47100, 300001) }.isFailure)
    }
    @Test fun sameBirthReloadRevisionInvalidatesInputAndOldReturnCannotPublishB() {
        val w = DebugAppliedInputWitness(); val a = Any(); val server = Any()
        val first = w.begin(a, server, "run", "birth", "old")
        w.returned(first, true); assertFalse(w.snapshot().startAcknowledged)
        w.acknowledge(a, server, true); assertTrue(w.snapshot().startAcknowledged)
        val reload = w.begin(a, server, "run", "birth", "new")
        assertEquals("NativeInputInFlight", w.snapshot().stage); assertTrue(reload.revision > first.revision)
        w.failed(reload); assertEquals("Unknown", w.snapshot().stage)
        val b = Any(); val next = w.begin(b, Any(), "run-b", "birth-b", "b")
        w.returned(first, true); w.seal(a); assertEquals(next.revision, w.snapshot().revision)
        assertEquals("NativeInputInFlight", w.snapshot().stage)
        w.returned(next, false); assertEquals("Unknown", w.snapshot().stage)
    }
    @Test fun lateAcquireCloseIsActuallyWaitedAndCloseFailureIsSticky() {
        val lease = DebugBatchLease(); val entered = CountDownLatch(1); val release = CountDownLatch(1)
        assertTrue(lease.workerBorn()); assertTrue(lease.beginAcquire()); lease.seal()
        val thread = Thread { try { lease.publish(Closeable { entered.countDown(); check(release.await(3, TimeUnit.SECONDS)) }) } finally { lease.workerReturned() } }
        thread.start(); assertTrue(entered.await(3, TimeUnit.SECONDS))
        assertEquals(1, lease.snapshot().closing); assertEquals(1, lease.snapshot().workers)
        release.countDown(); thread.join(3000); assertFalse(thread.isAlive)
        assertEquals(0, lease.snapshot().closing); assertEquals(0, lease.snapshot().workers)
        val failed = DebugBatchLease(); failed.beginAcquire(); failed.publish(Closeable { error("close failure") }); failed.seal(); failed.seal()
        assertTrue(failed.snapshot().closeFailed)
    }
    @Test fun partialBindAndRejectedDispatchRetainExactHandlesWithoutSelfJoin() {
        val lease = DebugBatchLease(); var closed = 0
        lease.workerBorn(); lease.beginAcquire(); lease.publish(Closeable { closed++ })
        lease.beginAcquire(); lease.acquireFailed() // second factory/bind fails
        lease.seal(); lease.workerReturned() // executor rejection or same worker's finally
        assertEquals(1, closed); assertEquals(0, lease.snapshot().workers)
        assertFalse(lease.workerBorn()); assertFalse(lease.beginAcquire())
    }
    @Test fun queuedCancellationReturnsAdmissionButRunningCancellationWaitsActualFinally() {
        val lease = DebugBatchLease(); lease.workerBorn(); var ran = false
        val queued = DebugBatchTask(lease, { ran = true }, { throw it })
        queued.rejectBeforeRun(); queued.run(); assertFalse(ran); assertEquals(0, lease.snapshot().workers)
        val entered = CountDownLatch(1); val release = CountDownLatch(1)
        lease.workerBorn()
        val running = DebugBatchTask(lease, { entered.countDown(); check(release.await(3, TimeUnit.SECONDS)) }, { throw it })
        val thread = Thread(running); thread.start(); assertTrue(entered.await(3, TimeUnit.SECONDS))
        running.rejectBeforeRun(); assertEquals(1, lease.snapshot().workers)
        release.countDown(); thread.join(3000); assertFalse(thread.isAlive); assertEquals(0, lease.snapshot().workers)
    }
    @Test fun actualMainConstructionLockAndRevocationKeepLateJniReturnUnknown() {
        val ledger = AndroidNativeAdmission("qa-input-seam").also { it.bootstrap(RequiredMarkerProof.Absent) }
        val ticket = ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, "run-a")
        check(ledger.enterBirth(ticket))
        val attempt = MainKernelAttempt<Any>(runId = "run-a", nativeTicket = ticket)
        val server = Any(); val witness = DebugAppliedInputWitness()
        val entered = CountDownLatch(1); val release = CountDownLatch(1)
        val thread = Thread { runCatching {
            AndroidNativeMain.construct(ledger, attempt) {
                assertTrue(Thread.holdsLock(attempt.operationLock))
                val token = witness.begin(attempt, server, attempt.runId, attempt.birthNonce, "digest-a")
                entered.countDown(); check(release.await(3, TimeUnit.SECONDS))
                witness.returned(token, !attempt.revoked)
            }
        } }
        thread.start(); assertTrue(entered.await(3, TimeUnit.SECONDS))
        witness.seal(attempt); attempt.revokeAndDetachTun(); release.countDown()
        thread.join(3000); assertFalse(thread.isAlive)
        assertEquals("Unknown", witness.snapshot().stage)
        assertFalse(witness.snapshot().startAcknowledged)
        assertTrue(attempt.constructionUnknown) // original production coordinator retained its failure
    }
    @Test fun blockedSdkSampleExpiresWithoutWaitingAndControllerCannotExtendDeadline() {
        assertNull(DebugBatchGuard.abortReason(1500, 1000, 9000, 5000, true, null, null))
        assertEquals("platform-signal-unavailable", DebugBatchGuard.abortReason(3000, 1000, 9000, 5000, true, null, null))
        assertTrue(DebugBatchGuard.platformReady(2500, 1500, true))
        assertFalse(DebugBatchGuard.platformReady(3500, 1500, true))
        assertFalse(DebugBatchGuard.platformReady(1200, 1500, true))
        assertEquals("platform-signal-unavailable", DebugBatchGuard.abortReason(1500, 1000, 9000, 5000, true, 1500, false))
        assertEquals("bounded-guard-abort", DebugBatchGuard.abortReason(5000, 1000, 9000, 5000, true, 4900, true))
        assertEquals("bounded-guard-abort", DebugBatchGuard.abortReason(9000, 1000, 9000, 12000, true, 8900, true))
        assertEquals("bounded-guard-abort", DebugBatchGuard.abortReason(1500, 1000, 9000, 5000, false, 1500, true))
    }
}
