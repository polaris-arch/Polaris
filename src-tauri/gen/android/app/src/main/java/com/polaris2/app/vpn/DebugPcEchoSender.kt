package com.polaris2.app.vpn

import android.os.SystemClock
import com.polaris2.app.BuildConfig
import java.io.Closeable
import java.io.EOFException
import java.io.InputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.net.SocketTimeoutException
import java.util.Base64
import org.json.JSONArray
import org.json.JSONObject

/** Actual sockets only reach the loopback ingress. No host/factory/URL comes from JS. */
internal object DebugPcEchoSender {
    data class Tuple(val address: String, val port: Int, val protocol: String) {
        fun json() = JSONObject().put("address", address).put("port", port).put("protocol", protocol)
    }
    private val loopback = byteArrayOf(127, 0, 0, 1)
    internal fun socksRequest(tuple: Tuple, nonce: ByteArray): ByteArray {
        require(DebugBatchProtocol.literalLanIPv4(tuple.address) && tuple.protocol == "udp" && tuple.port in 49152..65535 && nonce.size in 32..64)
        val ip = tuple.address.split('.').map { it.toInt().toByte() }.toByteArray()
        return byteArrayOf(0, 0, 0, 1) + ip + byteArrayOf((tuple.port ushr 8).toByte(), tuple.port.toByte()) + nonce
    }
    internal fun socksPayload(raw: ByteArray, tuple: Tuple): ByteArray? {
        if (raw.size !in 10..1500 || raw[0] != 0.toByte() || raw[1] != 0.toByte() || raw[2] != 0.toByte() || raw[3] != 1.toByte()) return null
        val address = (4..7).joinToString(".") { (raw[it].toInt() and 255).toString() }
        val port = (raw[8].toInt() and 255) * 256 + (raw[9].toInt() and 255)
        return if (address == tuple.address && port == tuple.port) raw.copyOfRange(10, raw.size) else null
    }
    internal fun approved(target: DebugPcEchoTarget, tuple: Tuple): Boolean = tuple.protocol in setOf("tcp", "udp") &&
        tuple.address == target.address && tuple.port == target.port(tuple.protocol)

    private class Ledger(val target: DebugPcEchoTarget, val caseId: String, val kind: String,
                         val sequence: Int, val tuple: Tuple) {
        var sent: ByteArray? = null
        var returned: ByteArray? = null
        var outcome = "NotObserved"
        fun publicJson(): JSONObject {
            fun bytes(value: ByteArray?, absent: String) = if (value == null) JSONObject().put("kind", absent)
                else JSONObject().put("kind", "Bytes").put("count", value.size).put("sha256", DebugPcEchoCodec.sha(value))
            return JSONObject().put("schema", "polaris-pc-echo-public-witness-v1")
                .put("pcRunId", target.fields["pcRunId"]).put("pcPlanSha256", target.fields["pcPlanSha256"])
                .put("readyReceiptSha256", target.fields["readyReceiptSha256"])
                .put("receiverInstanceId", target.fields["receiverInstanceId"]).put("socketInstanceId", target.socketId(tuple.protocol))
                .put("protocol", tuple.protocol).put("caseId", caseId).put("caseKind", kind).put("attemptSeq", sequence)
                .put("requestedTuple", tuple.json()).put("approvedTuple", Tuple(target.address, target.port(tuple.protocol), tuple.protocol).json())
                .put("sent", bytes(sent, "NotSent")).put("returned", bytes(returned, "NotReceived"))
                .put("outcome", outcome).put("rootNativeWitness", JSONObject().put("kind", "Unknown"))
        }
        fun erase() { sent?.fill(0); returned?.fill(0) }
    }
    private fun <T : Closeable> acquire(lease: DebugBatchLease, construct: () -> T): T {
        check(lease.beginAcquire())
        val value = try { construct() } catch (failure: Throwable) { lease.acquireFailed(); throw failure }
        check(lease.publish(value)); return value
    }
    private fun remaining(deadline: Long): Int = (deadline - SystemClock.elapsedRealtime()).coerceAtMost(1000).toInt().also { check(it > 0) }
    private fun secret(lease: DebugBatchLease, bytes: ByteArray): DebugCoreProbeCredentialBuffer =
        try { acquire(lease) { DebugCoreProbeCredentialBuffer(bytes) } }
        catch (failure: Throwable) { bytes.fill(0); throw failure }
    private fun current(loan: DebugCoreProbeLoan, ready: DebugPcEchoReady, allowed: () -> Boolean) {
        val now = SystemClock.elapsedRealtime()
        check(allowed() && ready.current(now) && loan.isCurrent(now, DebugCoreProbeLoan.currentInput()))
    }
    /** Handshake EOF is always a transport failure, including a negative attempt. */
    internal fun readHandshake(input: InputStream, count: Int, fence: () -> Unit): ByteArray {
        val out = ByteArray(count); var used = 0
        while (used < count) {
            fence()
            val n = input.read(out, used, count - used)
            if (n < 0) throw EOFException("PC echo handshake ended")
            check(n > 0); used += n
        }
        return out
    }
    private fun readExact(socket: Socket, count: Int, deadline: Long, fence: () -> Unit): ByteArray =
        readHandshake(socket.getInputStream(), count) { fence(); socket.soTimeout = remaining(deadline) }
    /** Only the fully sent WrongNonce echo phase may treat clean EOF as expected refusal. */
    internal fun readTcpEcho(input: InputStream, payload: ByteArray, kind: String, fullySent: Boolean,
                             fence: () -> Unit, progress: (ByteArray) -> Unit): String {
        val echoed = ArrayList<Byte>()
        while (echoed.size < 65) {
            fence()
            val value = input.read()
            if (value < 0) {
                fence()
                if (kind == "WrongNonce" && fullySent) return "NoEcho"
                throw EOFException("PC echo ended")
            }
            val b = value.toByte(); echoed.add(b); progress(echoed.toByteArray())
            if (b == 10.toByte()) break
        }
        return if (echoed.toByteArray().contentEquals(payload)) "ExactEcho" else "NoEcho"
    }
    /** Bytewise writes preserve exactly confirmed progress, including cancellation after send. */
    private fun write(socket: Socket, raw: ByteArray, fence: () -> Unit, progress: ((Int) -> Unit)? = null) {
        val output = socket.getOutputStream()
        for (index in raw.indices) { fence(); output.write(raw[index].toInt() and 255); progress?.invoke(index + 1) }
        output.flush()
    }
    private fun tcp(loan: DebugCoreProbeLoan, ready: DebugPcEchoReady, lease: DebugBatchLease,
                    tuple: Tuple, password: ByteArray, payload: ByteArray, report: Ledger, allowed: () -> Boolean) {
        val deadline = minOf(loan.deadlineElapsed, ready.deadlineElapsed)
        val socket = acquire(lease) { Socket() }
        val privateBuffers = mutableListOf<DebugCoreProbeCredentialBuffer>()
        val fence = { current(loan, ready, allowed) }
        try {
            fence(); socket.connect(InetSocketAddress(InetAddress.getByAddress(loopback), loan.probePort), remaining(deadline))
            // Authentication is a private transient byte buffer, never a String/log/report.
            val credentials = "polaris:".toByteArray(Charsets.US_ASCII) + password
            val credentialHandle = secret(lease, credentials).also(privateBuffers::add)
            val token = Base64.getEncoder().encode(credentials)
            val tokenHandle = secret(lease, token).also(privateBuffers::add)
            val header = ("CONNECT ${tuple.address}:${tuple.port} HTTP/1.1\r\nHost: ${tuple.address}:${tuple.port}\r\nProxy-Authorization: Basic ").toByteArray(Charsets.US_ASCII) + token + "\r\n\r\n".toByteArray(Charsets.US_ASCII)
            val headerHandle = secret(lease, header).also(privateBuffers::add)
            try { write(socket, header, fence) } finally { lease.retire(headerHandle); lease.retire(tokenHandle); lease.retire(credentialHandle) }
            val response = ArrayList<Byte>(); var tail = 0
            while (tail != 0x0d0a0d0a) {
                require(response.size < 4096); val b = readExact(socket, 1, deadline, fence)[0]
                response.add(b); tail = (tail shl 8) or (b.toInt() and 255)
            }
            val status = response.toByteArray().toString(Charsets.US_ASCII).substringBefore("\r\n")
            require(status.matches(Regex("HTTP/1\\.[01] 200(?: .*)?")))
            write(socket, payload, fence) { confirmed -> report.sent = payload.copyOfRange(0, confirmed) }
            report.outcome = readTcpEcho(socket.getInputStream(), payload, report.kind,
                report.sent?.contentEquals(payload) == true,
                { fence(); socket.soTimeout = remaining(deadline) }, { report.returned = it })
        } finally { privateBuffers.forEach(lease::retire); lease.retire(socket) }
    }
    private fun udp(loan: DebugCoreProbeLoan, ready: DebugPcEchoReady, lease: DebugBatchLease,
                    tuple: Tuple, password: ByteArray, payload: ByteArray, report: Ledger, allowed: () -> Boolean) {
        require(loan.supportsUdp)
        val deadline = minOf(loan.deadlineElapsed, ready.deadlineElapsed)
        val control = acquire(lease) { Socket() }
        var datagram: DatagramSocket? = null
        val fence = { current(loan, ready, allowed); check(!control.isClosed) }
        try {
            fence(); control.connect(InetSocketAddress(InetAddress.getByAddress(loopback), loan.probePort), remaining(deadline))
            write(control, byteArrayOf(5, 1, 2), fence)
            require(readExact(control, 2, deadline, fence).contentEquals(byteArrayOf(5, 2)))
            val user = "polaris".toByteArray(Charsets.US_ASCII)
            val auth = byteArrayOf(1, user.size.toByte()) + user + byteArrayOf(password.size.toByte()) + password
            val authHandle = secret(lease, auth)
            try { write(control, auth, fence) } finally { lease.retire(authHandle) }
            require(readExact(control, 2, deadline, fence).contentEquals(byteArrayOf(1, 0)))
            // ASSOCIATE stays bound to this original control socket through send/read/finally.
            write(control, byteArrayOf(5, 3, 0, 1, 0, 0, 0, 0, 0, 0), fence)
            val head = readExact(control, 4, deadline, fence)
            require(head.contentEquals(byteArrayOf(5, 0, 0, 1))) // No domain/DNS or unbounded relay.
            val relay = readExact(control, 6, deadline, fence)
            require(relay.copyOfRange(0, 4).contentEquals(loopback) || relay.copyOfRange(0, 4).contentEquals(ByteArray(4)))
            val relayPort = (relay[4].toInt() and 255) * 256 + (relay[5].toInt() and 255); require(relayPort in 1..65535)
            val relayFence = {
                fence()
                // UDP ASSOCIATE control has no subsequent application bytes. A bounded
                // actual EOF check rejects a remotely closed association as well as local Close.
                control.soTimeout = 1
                try { control.getInputStream().read(); error("UDP association unavailable") }
                catch (_: SocketTimeoutException) { /* original control remains open */ }
            }
            datagram = acquire(lease) { DatagramSocket(null as java.net.SocketAddress?) }
            val socket = datagram
            relayFence(); socket.bind(InetSocketAddress(InetAddress.getByAddress(loopback), 0))
            relayFence(); socket.connect(InetAddress.getByAddress(loopback), relayPort)
            val packet = socksRequest(tuple, payload)
            try {
                relayFence(); socket.send(DatagramPacket(packet, packet.size)); report.sent = payload.copyOf()
            } finally { packet.fill(0) }
            socket.soTimeout = remaining(deadline)
            // One extra byte identifies oversize; report only the bounded captured prefix.
            val buffer = ByteArray(1501); val response = DatagramPacket(buffer, buffer.size)
            relayFence(); socket.receive(response)
            require(response.address.address.contentEquals(loopback) && response.port == relayPort && response.length <= 1500)
            val body = socksPayload(buffer.copyOf(response.length), tuple)
            require(body != null)
            report.returned = body
            relayFence()
            report.outcome = if (body.contentEquals(payload)) "ExactEcho" else "NoEcho"
        } finally { datagram?.let(lease::retire); lease.retire(control) }
    }

    /** Six fixed groups, no retries. Foreign-peer needs an external actor and is honest Unsupported. */
    fun run(loan: DebugCoreProbeLoan, ready: DebugPcEchoReady, lease: DebugBatchLease,
            channel: DebugPcEchoChannel, allowed: () -> Boolean): String {
        check(BuildConfig.DEBUG) { "Debug PC echo is disabled" }
        val witnesses = JSONArray(); val snapshots = JSONArray(); var sequence = 0
        var complete = true
        try {
            try { loan.consume(SystemClock.elapsedRealtime(), DebugCoreProbeLoan.currentInput()) { password ->
                for (protocol in listOf("tcp", "udp")) for (group in listOf("wrong-nonce", "foreign-peer", "wrong-port-admission")) {
                    val caseId = "$protocol-$group"
                    for ((position, kind) in listOf("before" to "Positive", "negative" to when (group) {
                        "wrong-nonce" -> "WrongNonce"; "foreign-peer" -> "ForeignPeer"; else -> "WrongPortAdmission"
                    }, "after" to "Positive")) {
                        current(loan, ready, allowed)
                        snapshots.put(JSONObject(DebugPcEchoCodec.canonical(channel.snapshot("$caseId-$position")).toString(Charsets.UTF_8)))
                        val approved = Tuple(ready.target.address, ready.target.port(protocol), protocol)
                        val tuple = if (kind == "WrongPortAdmission") approved.copy(port = if (approved.port == 65535) 65534 else approved.port + 1) else approved
                        val report = Ledger(ready.target, caseId, kind, ++sequence, tuple)
                        try {
                            if (!approved(ready.target, tuple)) report.outcome = "RejectedBeforeOutbound"
                            else if (kind == "ForeignPeer" || protocol == "udp" && !loan.supportsUdp) {
                                report.outcome = "Unsupported"; complete = false
                            } else {
                                val nonce = ready.target.nonce.copyOf()
                                if (kind == "WrongNonce") nonce[0] = if (nonce[0] == 'a'.code.toByte()) 'b'.code.toByte() else 'a'.code.toByte()
                                val payload = if (protocol == "tcp") nonce + byteArrayOf(10) else nonce
                                try {
                                    if (protocol == "tcp") tcp(loan, ready, lease, tuple, password, payload, report, allowed)
                                    else udp(loan, ready, lease, tuple, password, payload, report, allowed)
                                } catch (_: SocketTimeoutException) { report.outcome = "NoEcho" }
                                catch (_: Throwable) {
                                    report.outcome = if (!ready.current(SystemClock.elapsedRealtime()) || !allowed()) "Canceled"
                                        else if (!loan.isCurrent(SystemClock.elapsedRealtime(), DebugCoreProbeLoan.currentInput())) "StaleScope" else "TransportFailure"
                                } finally { nonce.fill(0); payload.fill(0) }
                                // Already sent bytes remain present if the final scope fence fails.
                                if (!allowed() || !ready.current(SystemClock.elapsedRealtime())) report.outcome = "Canceled"
                                else if (!loan.isCurrent(SystemClock.elapsedRealtime(), DebugCoreProbeLoan.currentInput())) report.outcome = "StaleScope"
                            }
                            witnesses.put(report.publicJson())
                            if (report.outcome in setOf("Canceled", "StaleScope", "TransportFailure")) error("Batch input unavailable")
                        } finally { report.erase() }
                    }
                    snapshots.put(JSONObject(DebugPcEchoCodec.canonical(channel.snapshot("$caseId-final")).toString(Charsets.UTF_8)))
                }
            } } catch (_: Throwable) { complete = false }
            return JSONObject().put("schema", "polaris-debug-pc-echo-batch-v1").put("witnesses", witnesses)
                .put("receiverSnapshots", snapshots).put("foreignActor", "Incomplete")
                .put("sourceStatus", "ImplementedAwaitingUnifiedValidation").put("complete", complete)
                .put("coreProbePath", "Unknown").toString()
        } finally { lease.retire(ready); lease.retire(loan) }
    }
}
