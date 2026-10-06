package com.polaris2.app.vpn

import com.polaris2.app.BuildConfig
import java.io.ByteArrayInputStream
import java.io.Closeable
import java.io.EOFException
import java.io.IOException
import java.io.InputStream
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
        val retained = "\"echoNonce\":\"" + "b".repeat(32) + "\""
        val live = DebugPcEchoCodec.parse(ready.target.privateJson().toByteArray())
        assertEquals(DebugPcEchoTarget.KEYS, live.keys); assertEquals("b".repeat(32), live["echoNonce"])
        assertFalse(ready.target.fields.containsKey("echoNonce"))
        val blocker = Closeable {
            check(!ready.target.fields.containsKey("echoNonce") && !ready.target.privateJson().contains(retained))
            entered.countDown(); check(release.await(3, TimeUnit.SECONDS))
        }
        check(lease.beginAcquire()); check(lease.publish(blocker)); check(lease.beginAcquire()); check(lease.publish(ready))
        val worker = Thread { lease.seal() }; worker.start(); assertTrue(entered.await(3, TimeUnit.SECONDS))
        try {
            assertTrue(ready.target.nonce.all { it == 0.toByte() }); assertFalse(ready.current(1400))
            assertFalse(ready.target.privateJson().contains(retained))
            // Even refilling the old byte array cannot restore a revoked private descriptor.
            ready.target.nonce.fill('b'.code.toByte())
            assertFalse(ready.target.privateJson().contains(retained)); ready.target.nonce.fill(0)
        } finally { release.countDown(); worker.join(3000) }
        assertFalse(worker.isAlive); assertEquals(0, lease.snapshot().handles)
        assertFalse(ready.target.privateJson().contains(retained))
    }
    @Test fun actualTcpEchoReaderAcceptsOnlyFullySentWrongNonceEofAndPreservesProgress() {
        val payload = "b".repeat(32).toByteArray() + byteArrayOf(10)
        var returned: ByteArray? = null; var fences = 0
        fun echo(raw: ByteArray, kind: String = "Positive", sent: Boolean = true): String =
            DebugPcEchoSender.readTcpEcho(ByteArrayInputStream(raw), payload, kind, sent,
                { fences++ }, { returned = it })
        assertEquals("ExactEcho", echo(payload))
        returned = null
        assertEquals("NoEcho", echo(byteArrayOf(), "WrongNonce")); assertNull(returned)
        val partial = payload.copyOfRange(0, 3)
        assertEquals("NoEcho", echo(partial, "WrongNonce")); assertArrayEquals(partial, returned)
        assertTrue(runCatching { echo(byteArrayOf()) }.exceptionOrNull() is EOFException)
        assertTrue(runCatching { echo(partial) }.exceptionOrNull() is EOFException)
        assertTrue(runCatching { echo(byteArrayOf(), "WrongNonce", false) }.exceptionOrNull() is EOFException)
        assertTrue(runCatching { echo(byteArrayOf(), "ForeignPeer") }.exceptionOrNull() is EOFException)
        // Actual reader resumes after the expected negative; UDP uses its unchanged closed codec.
        assertEquals("ExactEcho", echo(payload)); assertArrayEquals(payload, returned); assertTrue(fences > 0)
        val target = DebugPcEchoTarget.decode(target())
        val tuple = DebugPcEchoSender.Tuple(target.address, target.port("udp"), "udp")
        val packet = DebugPcEchoSender.socksRequest(tuple, target.nonce)
        assertArrayEquals(target.nonce, DebugPcEchoSender.socksPayload(packet, tuple))
        packet.fill(0); target.erase(); payload.fill(0); returned?.fill(0)
    }
    @Test fun actualTcpReadersKeepHandshakeIoAndCancellationFailures() {
        assertArrayEquals(byteArrayOf(5, 2), DebugPcEchoSender.readHandshake(ByteArrayInputStream(byteArrayOf(5, 2)), 2) {})
        for (raw in listOf(byteArrayOf(), byteArrayOf(5)))
            assertTrue(runCatching { DebugPcEchoSender.readHandshake(ByteArrayInputStream(raw), 2) {} }.exceptionOrNull() is EOFException)
        val io = IOException("synthetic read failure")
        val broken = object : InputStream() { override fun read(): Int = throw io }
        assertSame(io, runCatching { DebugPcEchoSender.readHandshake(broken, 1) {} }.exceptionOrNull())
        for (kind in listOf("Positive", "WrongNonce"))
            assertSame(io, runCatching { DebugPcEchoSender.readTcpEcho(broken, byteArrayOf(10), kind, true, {}, {}) }.exceptionOrNull())
        val canceled = IllegalStateException("synthetic revoked scope")
        var fences = 0
        assertSame(canceled, runCatching {
            DebugPcEchoSender.readTcpEcho(ByteArrayInputStream(byteArrayOf()), byteArrayOf(10), "WrongNonce", true,
                { if (++fences == 2) throw canceled }, {})
        }.exceptionOrNull())
    }

    private fun wrongPortWitness(tcpPort: Int, udpPort: Int) {
        assumeTrue(BuildConfig.DEBUG)
        val target = DebugPcEchoTarget.decode(target() + mapOf("tcpPort" to tcpPort.toLong(), "udpPort" to udpPort.toLong()))
        val lease = DebugBatchLease(); val initialLease = lease.snapshot()
        try {
            for ((index, protocol) in listOf("tcp", "udp").withIndex()) {
                val caseId = "$protocol-wrong-port-admission"; val sequence = index + 1
                var outboundCalls = 0
                var witness: org.json.JSONObject? = null
                DebugPcEchoSender.attempt(target, protocol, caseId, "WrongPortAdmission", sequence,
                    { _, _ -> outboundCalls++; fail("WrongPort reached outbound resource acquisition or send") },
                    { assertNull(witness); witness = it })
                assertEquals(0, outboundCalls); assertEquals(initialLease, lease.snapshot())
                val expected = mapOf(
                    "schema" to "polaris-pc-echo-public-witness-v1", "pcRunId" to "1".repeat(32),
                    "pcPlanSha256" to "2".repeat(64), "readyReceiptSha256" to "3".repeat(64),
                    "receiverInstanceId" to "8".repeat(32), "socketInstanceId" to target.socketId(protocol),
                    "protocol" to protocol, "caseId" to caseId, "caseKind" to "WrongPortAdmission", "attemptSeq" to sequence,
                    "requestedTuple" to mapOf("address" to target.address, "port" to target.port(if (protocol == "tcp") "udp" else "tcp"), "protocol" to protocol),
                    "approvedTuple" to mapOf("address" to target.address, "port" to target.port(protocol), "protocol" to protocol),
                    "sent" to mapOf("kind" to "NotSent"), "returned" to mapOf("kind" to "NotReceived"),
                    "outcome" to "RejectedBeforeOutbound", "rootNativeWitness" to mapOf("kind" to "Unknown"))
                assertEquals("actual production witness for $protocol/$tcpPort/$udpPort", expected, witness!!.toMap())
            }
        } finally { target.erase() }
    }
    @Test fun actualWrongPortAdmissionUsesNonAdjacentOtherApprovedPortInBothDirections() {
        wrongPortWitness(50001, 51017)
    }
    @Test fun actualWrongPortAdmissionUsesReversedOtherApprovedPortInBothDirections() {
        wrongPortWitness(61000, 50003)
    }
    @Test fun actualWrongPortAdmissionUsesBothRangeBoundariesWithoutArithmetic() {
        wrongPortWitness(49152, 65535); wrongPortWitness(65535, 49152)
    }
    @Test fun actualWrongPortAdmissionDoesNotHideUdpBehindAdjacentTcpFixture() {
        wrongPortWitness(50001, 50002)
    }
    @Test fun actualAdmissionRejectsUnknownProtocolBeforeEitherCallbackAndStrictTargetKeepsPortBounds() {
        assumeTrue(BuildConfig.DEBUG)
        val target = DebugPcEchoTarget.decode(target())
        try {
            for (kind in listOf("Positive", "WrongPortAdmission")) {
                var outboundCalls = 0; var appendCalls = 0
                val failure = runCatching {
                    DebugPcEchoSender.attempt(target, "sctp", "sctp-wrong-port-admission", kind, 1,
                        { _, _ -> outboundCalls++ }, { appendCalls++ })
                }.exceptionOrNull()
                assertTrue(failure is IllegalArgumentException)
                assertEquals(0, outboundCalls); assertEquals(0, appendCalls)
            }
            for (ports in listOf(50001L to 50001L, 49151L to 50002L, 50001L to 65536L))
                assertTrue(runCatching { DebugPcEchoTarget.decode(target() + mapOf("tcpPort" to ports.first, "udpPort" to ports.second)) }.isFailure)
        } finally { target.erase() }
    }
    @Test fun actualAdmissionKeepsOrdinaryTupleAndErasesProgressAfterPublicWitness() {
        assumeTrue(BuildConfig.DEBUG)
        val target = DebugPcEchoTarget.decode(target())
        try {
            for (protocol in listOf("tcp", "udp")) for (kind in listOf("Positive", "WrongNonce", "ForeignPeer")) {
                var calls = 0; var appended = 0
                val sent = byteArrayOf(1, 2, 3); val returned = byteArrayOf(4, 5)
                val sentSha = DebugPcEchoCodec.sha(sent); val returnedSha = DebugPcEchoCodec.sha(returned)
                DebugPcEchoSender.attempt(target, protocol, "$protocol-$kind", kind, 1, { tuple, report ->
                    calls++; assertEquals(DebugPcEchoSender.Tuple(target.address, target.port(protocol), protocol), tuple)
                    report.sent = sent; report.returned = returned; report.outcome = "ExactEcho"
                }, { witness ->
                    appended++; assertEquals(kind, witness.getString("caseKind")); assertEquals("ExactEcho", witness.getString("outcome"))
                    assertEquals(witness.getJSONObject("approvedTuple").toMap(), witness.getJSONObject("requestedTuple").toMap())
                    assertEquals(mapOf("kind" to "Bytes", "count" to 3, "sha256" to sentSha), witness.getJSONObject("sent").toMap())
                    assertEquals(mapOf("kind" to "Bytes", "count" to 2, "sha256" to returnedSha), witness.getJSONObject("returned").toMap())
                    assertArrayEquals(byteArrayOf(1, 2, 3), sent); assertArrayEquals(byteArrayOf(4, 5), returned)
                })
                assertEquals(1, calls); assertEquals(1, appended)
                assertArrayEquals(ByteArray(3), sent); assertArrayEquals(ByteArray(2), returned)
            }
        } finally { target.erase() }
    }
    @Test fun actualAdmissionAppendsTerminalProgressBeforeErrorAndAlwaysErasesIt() {
        assumeTrue(BuildConfig.DEBUG)
        val target = DebugPcEchoTarget.decode(target())
        try {
            for (outcome in listOf("Canceled", "StaleScope", "TransportFailure")) {
                val sent = byteArrayOf(1, 2, 3); val returned = byteArrayOf(4)
                val sentSha = DebugPcEchoCodec.sha(sent); var appended = 0
                val failure = runCatching {
                    DebugPcEchoSender.attempt(target, "tcp", "tcp-wrong-nonce", "WrongNonce", 2,
                        { _, report -> report.sent = sent; report.returned = returned; report.outcome = outcome },
                        { witness ->
                            appended++; assertEquals(outcome, witness.getString("outcome"))
                            assertEquals(sentSha, witness.getJSONObject("sent").getString("sha256"))
                            assertArrayEquals(byteArrayOf(1, 2, 3), sent); assertArrayEquals(byteArrayOf(4), returned)
                        })
                }.exceptionOrNull()
                assertTrue(failure is IllegalStateException); assertEquals("Batch input unavailable", failure!!.message)
                assertEquals(1, appended); assertArrayEquals(ByteArray(3), sent); assertArrayEquals(ByteArray(1), returned)
            }
        } finally { target.erase() }
    }
    @Test fun actualAdmissionDoesNotSwallowOrRetryOutboundOrAppendCallbackFailure() {
        assumeTrue(BuildConfig.DEBUG)
        val target = DebugPcEchoTarget.decode(target())
        try {
            for (failOnAppend in listOf(false, true)) {
                val failure = IOException("synthetic callback failure")
                val sent = byteArrayOf(1, 2); val returned = byteArrayOf(3)
                var calls = 0; var appended = 0
                assertSame(failure, runCatching {
                    DebugPcEchoSender.attempt(target, "udp", "udp-wrong-nonce", "WrongNonce", 2, { _, report ->
                        calls++; report.sent = sent; report.returned = returned
                        if (!failOnAppend) throw failure
                    }, { appended++; throw failure })
                }.exceptionOrNull())
                assertEquals(1, calls); assertEquals(if (failOnAppend) 1 else 0, appended)
                assertArrayEquals(ByteArray(2), sent); assertArrayEquals(ByteArray(1), returned)
            }
        } finally { target.erase() }
    }
}
