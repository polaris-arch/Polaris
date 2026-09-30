package com.polaris2.app.vpn

import com.polaris2.app.BuildConfig
import java.io.Closeable
import java.util.Base64
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test

/** LogicOnly fixtures. No Android SDK, socket, PC issuer, JNI or device execution. */
class DebugPcEchoTest {
    private fun target(): Map<String, Any?> = mapOf(
        "schema" to "polaris-pc-echo-target-v1", "pcRunId" to "1".repeat(32), "pcPlanSha256" to "2".repeat(64),
        "readyReceiptSha256" to "3".repeat(64), "pcCandidateSha" to "4".repeat(40), "androidCandidateSha" to "5".repeat(40),
        "receiverSourceSha256" to "6".repeat(64), "androidPackageSha256" to "7".repeat(64),
        "receiverInstanceId" to "8".repeat(32), "tcpSocketInstanceId" to "9".repeat(32), "udpSocketInstanceId" to "a".repeat(32),
        "destinationIPv4" to "192.168.1.1", "expectedSenderIPv4" to "192.168.1.2", "tcpPort" to 50001L,
        "udpPort" to 50002L, "echoNonce" to "b".repeat(32), "echoNonceSha256" to DebugPcEchoCodec.sha("b".repeat(32).toByteArray()),
        "lifetimeSeconds" to 120L, "maxRequests" to 32L)
    private fun reply(): Map<String, Any?> {
        val original = mapOf("phase" to "Ready", "sourceKind" to "OperatorDeclared", "binding" to mapOf("runId" to "1".repeat(32)),
            "pcPlanSha256" to "2".repeat(64), "controllerInstanceId" to "c".repeat(32), "receiverInstanceId" to "8".repeat(32),
            "startedMonotonicNs" to 1000000L, "expiresMonotonicNs" to 120001000000L)
        val raw = DebugPcEchoCodec.canonical(original)
        val handoff = original + mapOf("phase" to "ReadyHandoff", "requestId" to "d".repeat(32), "readyReceiptSha256" to DebugPcEchoCodec.sha(raw),
            "tcpSocketInstanceId" to "9".repeat(32), "udpSocketInstanceId" to "a".repeat(32), "remainingLifetimeMs" to 2000L, "remainingRequests" to 32L)
        return mapOf("schema" to "polaris-debug-pc-echo-response-v1", "requestId" to "d".repeat(32), "action" to "ready",
            "target" to (target() + ("readyReceiptSha256" to DebugPcEchoCodec.sha(raw))), "remainingLifetimeMs" to 1800L,
            "remainingRequests" to 32L, "originalReady" to Base64.getEncoder().encodeToString(raw),
            "originalHandoff" to Base64.getEncoder().encodeToString(DebugPcEchoCodec.canonical(handoff)))
    }
    @Test fun strictClosedTargetAndUtf8RejectDuplicateUnknownBooleanAndPortAliases() {
        val valid = DebugPcEchoTarget.decode(DebugPcEchoCodec.parse(DebugPcEchoCodec.canonical(target())))
        assertEquals(50001, valid.port("tcp"))
        for (raw in listOf("{\"x\":1,\"x\":2}", "{\"x\":1.0}", "{\"x\":01}", "{\"x\":-0}", "{\"x\":\"\\ud800\"}"))
            assertTrue(runCatching { DebugPcEchoCodec.parse(raw.toByteArray()) }.isFailure)
        assertTrue(runCatching { DebugPcEchoCodec.parse(byteArrayOf(123,34,120,34,58,34,0xc0.toByte(),0x80.toByte(),34,125)) }.isFailure)
        for ((key, value) in listOf("extra" to true, "tcpPort" to true, "tcpPort" to 49151L, "udpPort" to 50001L,
            "destinationIPv4" to "192.168.01.1", "destinationIPv4" to "8.8.8.8", "maxRequests" to 31L,
            "echoNonceSha256" to "0".repeat(64), "tcpSocketInstanceId" to "a".repeat(32)))
            assertTrue(runCatching { DebugPcEchoTarget.decode(target() + (key to value)) }.isFailure)
        valid.nonce.fill(0)
    }
    @Test fun privateAuthenticationBindsCanonicalOriginalBodyAndSessionKey() {
        val key = ByteArray(32) { 9 }; val body = mapOf("requestId" to "a".repeat(32), "sequence" to 1L)
        val encoded = DebugPcEchoCodec.envelope(key, body)
        assertEquals(body, DebugPcEchoCodec.authenticated(key, encoded))
        assertTrue(runCatching { DebugPcEchoCodec.authenticated(ByteArray(32) { 8 }, encoded) }.isFailure)
        val changed = encoded.copyOf().also { it[it.size - 4] = 'f'.code.toByte() }
        assertTrue(runCatching { DebugPcEchoCodec.authenticated(key, changed) }.isFailure)
    }
    @Test fun originalRemainingLeaseUsesPreRequestAnchorAndNeverReceiptPlus120Seconds() {
        val good = DebugPcEchoReady.fromReply(reply(), "d".repeat(32), 1000, 1400, 9000, "7".repeat(64))
        assertEquals(2800, good.deadlineElapsed); assertFalse(good.current(2800))
        for (variant in 0..5) {
            val value = when (variant) {
                0 -> reply() + ("requestId" to "e".repeat(32))
                1 -> reply() + ("remainingLifetimeMs" to 2001L)
                2 -> reply() + ("remainingRequests" to 13L)
                3 -> reply() + ("remainingRequests" to true)
                4 -> reply() + ("target" to (target() + ("readyReceiptSha256" to "0".repeat(64))))
                else -> reply() + ("approved" to true)
            }
            assertTrue(runCatching { DebugPcEchoReady.fromReply(value, "d".repeat(32), 1000, 1400, 9000, "7".repeat(64)) }.isFailure)
        }
        assertTrue(runCatching { DebugPcEchoReady.fromReply(reply(), "d".repeat(32), 1000, 2800, 9000, "7".repeat(64)) }.isFailure)
        good.erase()
    }
    @Test fun socksFragAndReturnedDestinationAreExactAndWrongPortAdmissionIsPure() {
        val target = DebugPcEchoTarget.decode(target())
        val tuple = DebugPcEchoSender.Tuple(target.address, target.port("udp"), "udp")
        val packet = DebugPcEchoSender.socksRequest(tuple, target.nonce)
        assertArrayEquals(target.nonce, DebugPcEchoSender.socksPayload(packet, tuple))
        assertNull(DebugPcEchoSender.socksPayload(packet.copyOf().also { it[2] = 1 }, tuple))
        assertNull(DebugPcEchoSender.socksPayload(packet, tuple.copy(port = tuple.port + 1)))
        assertNull(DebugPcEchoSender.socksPayload(packet, tuple.copy(address = "192.168.1.3")))
        assertFalse(DebugPcEchoSender.approved(target, tuple.copy(port = tuple.port + 1)))
        assertTrue(DebugPcEchoSender.approved(target, tuple)); packet.fill(0); target.nonce.fill(0)
    }
    @Test fun originalLeaseErasesPrivateNonceBeforeAnyUnrelatedCloseCanBlock() {
        assumeTrue(BuildConfig.DEBUG)
        val lease = DebugBatchLease(); val ready = DebugPcEchoReady.fromReply(reply(), "d".repeat(32), 1000, 1400, 9000, "7".repeat(64))
        val entered = CountDownLatch(1); val release = CountDownLatch(1)
        val blocker = Closeable { entered.countDown(); check(release.await(3, TimeUnit.SECONDS)) }
        check(lease.beginAcquire()); check(lease.publish(blocker)); check(lease.beginAcquire()); check(lease.publish(ready))
        val worker = Thread { lease.seal() }; worker.start(); assertTrue(entered.await(3, TimeUnit.SECONDS))
        assertTrue(ready.target.nonce.all { it == 0.toByte() }); assertFalse(ready.current(1400))
        release.countDown(); worker.join(3000); assertFalse(worker.isAlive); assertEquals(0, lease.snapshot().handles)
    }
}
