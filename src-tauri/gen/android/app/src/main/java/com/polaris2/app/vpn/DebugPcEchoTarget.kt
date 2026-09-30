package com.polaris2.app.vpn

import android.app.Activity
import android.os.SystemClock
import com.polaris2.app.BuildConfig
import java.io.Closeable
import java.io.File
import java.nio.ByteBuffer
import java.nio.charset.CodingErrorAction
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec
import org.json.JSONObject

/** Closed private codec. JSONObject is only an output writer, never the input validator. */
internal object DebugPcEchoCodec {
    const val LIMIT = 65536
    fun sha(bytes: ByteArray) = DebugBatchProtocol.hex(MessageDigest.getInstance("SHA-256").digest(bytes))
    fun hex(value: String, minimum: Int, maximum: Int = minimum) =
        value.length in minimum..maximum && value.all { it in '0'..'9' || it in 'a'..'f' }
    fun parse(raw: ByteArray): Map<String, Any?> {
        require(raw.size in 2..LIMIT)
        val text = Charsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT)
            .onUnmappableCharacter(CodingErrorAction.REPORT).decode(ByteBuffer.wrap(raw)).toString()
        class Reader {
            var p = 0; var nodes = 0
            fun ws() { while (p < text.length && text[p] in " \t\r\n") p++ }
            fun string(): String {
                require(text[p++] == '"'); val result = StringBuilder()
                while (p < text.length) {
                    val c = text[p++]
                    if (c == '"') return result.toString()
                    require(c >= ' ' && !c.isSurrogate())
                    if (c != '\\') result.append(c) else {
                        require(p < text.length)
                        when (val e = text[p++]) {
                            '"', '\\', '/' -> result.append(e)
                            'b' -> result.append('\b'); 'f' -> result.append('\u000c')
                            'n' -> result.append('\n'); 'r' -> result.append('\r'); 't' -> result.append('\t')
                            'u' -> {
                                require(p + 4 <= text.length)
                                val n = text.substring(p, p + 4).toIntOrNull(16) ?: error("Private codec")
                                require(n !in 0xd800..0xdfff); result.append(n.toChar()); p += 4
                            }
                            else -> error("Private codec")
                        }
                    }
                    require(result.length <= LIMIT)
                }
                error("Private codec")
            }
            fun value(depth: Int): Any? {
                require(depth <= 8 && ++nodes <= 1024); ws(); require(p < text.length)
                return when (text[p]) {
                    '"' -> string()
                    '{' -> {
                        p++; ws(); val result = linkedMapOf<String, Any?>()
                        if (p < text.length && text[p] == '}') { p++; result } else {
                            while (true) {
                                ws(); require(p < text.length && text[p] == '"'); val key = string()
                                require(!result.containsKey(key)); ws(); require(p < text.length && text[p++] == ':')
                                result[key] = value(depth + 1); ws(); require(p < text.length)
                                val separator = text[p++]; if (separator == '}') break; require(separator == ',')
                            }; result
                        }
                    }
                    '[' -> {
                        p++; ws(); val result = mutableListOf<Any?>()
                        if (p < text.length && text[p] == ']') { p++; result } else {
                            while (true) {
                                result.add(value(depth + 1)); ws(); require(p < text.length)
                                val separator = text[p++]; if (separator == ']') break; require(separator == ',')
                            }; result
                        }
                    }
                    't' -> { require(text.startsWith("true", p)); p += 4; true }
                    'f' -> { require(text.startsWith("false", p)); p += 5; false }
                    'n' -> { require(text.startsWith("null", p)); p += 4; null }
                    else -> {
                        val start = p
                        if (text[p] == '-') p++
                        require(p < text.length && text[p] in '0'..'9')
                        if (text[p] == '0') p++ else while (p < text.length && text[p] in '0'..'9') p++
                        val number = text.substring(start, p).toLongOrNull() ?: error("Private codec")
                        require(number.toString() == text.substring(start, p)); number
                    }
                }
            }
        }
        val r = Reader(); val value = r.value(0); r.ws(); require(r.p == text.length && value is Map<*, *>)
        @Suppress("UNCHECKED_CAST") return value as Map<String, Any?>
    }
    fun objectValue(value: Any?, keys: Set<String>): Map<String, Any?> {
        require(value is Map<*, *> && value.keys == keys)
        @Suppress("UNCHECKED_CAST") return value as Map<String, Any?>
    }
    fun string(value: Any?) = (value as? String) ?: error("Private codec")
    fun integer(value: Any?, min: Long, max: Long): Long =
        (value as? Long)?.takeIf { it in min..max } ?: error("Private codec")
    fun canonical(value: Any?): ByteArray {
        fun encode(v: Any?): String = when (v) {
            is Map<*, *> -> v.entries.sortedBy { it.key as String }.joinToString(",", "{", "}") {
                JSONObject.quote(it.key as String) + ":" + encode(it.value)
            }
            is List<*> -> v.joinToString(",", "[", "]", transform = ::encode)
            is String -> JSONObject.quote(v)
            is Boolean, is Long, is Int -> v.toString()
            null -> "null"
            else -> error("Private codec")
        }
        return encode(value).toByteArray(Charsets.UTF_8).also { require(it.size <= LIMIT) }
    }
    fun mac(secret: ByteArray, body: ByteArray): ByteArray = Mac.getInstance("HmacSHA256").run {
        init(SecretKeySpec(secret, "HmacSHA256"))
        update("polaris-debug-pc-echo-private-v1\u0000".toByteArray(Charsets.US_ASCII)); doFinal(body)
    }
    fun envelope(secret: ByteArray, body: Map<String, Any?>): ByteArray {
        val raw = canonical(body)
        return canonical(mapOf("body" to Base64.getEncoder().encodeToString(raw), "mac" to DebugBatchProtocol.hex(mac(secret, raw))))
    }
    fun authenticated(secret: ByteArray, raw: ByteArray): Map<String, Any?> {
        val frame = objectValue(parse(raw), setOf("body", "mac"))
        val body = Base64.getDecoder().decode(string(frame["body"]))
        val supplied = string(frame["mac"]); require(hex(supplied, 64))
        require(MessageDigest.isEqual(mac(secret, body), DebugBatchProtocol.unhex(supplied)))
        return parse(body).also { require(canonical(it).contentEquals(body)) }
    }
}

/** Immutable actual receiver descriptor. Raw echo nonce remains private and erasable. */
internal class DebugPcEchoTarget private constructor(internal val fields: Map<String, Any?>,
                                                    internal val nonce: ByteArray) {
    private var erased = false
    val address get() = fields["destinationIPv4"] as String
    fun port(protocol: String) = (fields[protocol + "Port"] as Long).toInt()
    fun socketId(protocol: String) = fields[protocol + "SocketInstanceId"] as String
    val apkSha get() = fields["androidPackageSha256"] as String
    // This bounded record monitor holds only metadata/bytes, never SDK or IPC work.
    @Synchronized fun privateJson(): String = DebugPcEchoCodec.canonical(
        if (erased) fields else fields + ("echoNonce" to nonce.toString(Charsets.US_ASCII))
    ).toString(Charsets.UTF_8)
    @Synchronized fun erase() { erased = true; nonce.fill(0) }
    override fun toString() = "DebugPcEchoTarget(<private>)"
    companion object {
        val KEYS = setOf("schema", "pcRunId", "pcPlanSha256", "readyReceiptSha256", "pcCandidateSha",
            "androidCandidateSha", "receiverSourceSha256", "androidPackageSha256", "receiverInstanceId",
            "tcpSocketInstanceId", "udpSocketInstanceId", "destinationIPv4", "expectedSenderIPv4", "tcpPort",
            "udpPort", "echoNonce", "echoNonceSha256", "lifetimeSeconds", "maxRequests")
        fun decode(value: Any?): DebugPcEchoTarget {
            val m = DebugPcEchoCodec.objectValue(value, KEYS)
            fun s(k: String) = DebugPcEchoCodec.string(m[k])
            require(s("schema") == "polaris-pc-echo-target-v1")
            require(DebugPcEchoCodec.hex(s("pcRunId"), 32, 64))
            for (k in listOf("pcPlanSha256", "readyReceiptSha256", "receiverSourceSha256", "androidPackageSha256", "echoNonceSha256"))
                require(DebugPcEchoCodec.hex(s(k), 64))
            for (k in listOf("pcCandidateSha", "androidCandidateSha")) require(DebugPcEchoCodec.hex(s(k), 40))
            for (k in listOf("receiverInstanceId", "tcpSocketInstanceId", "udpSocketInstanceId")) require(DebugPcEchoCodec.hex(s(k), 32))
            require(s("tcpSocketInstanceId") != s("udpSocketInstanceId"))
            require(DebugBatchProtocol.literalLanIPv4(s("destinationIPv4")) && DebugBatchProtocol.literalLanIPv4(s("expectedSenderIPv4")))
            for (k in listOf("tcpPort", "udpPort")) DebugPcEchoCodec.integer(m[k], 49152, 65535)
            require(m["tcpPort"] != m["udpPort"])
            DebugPcEchoCodec.integer(m["lifetimeSeconds"], 1, 120)
            require(m["maxRequests"] == 32L && DebugPcEchoCodec.hex(s("echoNonce"), 32, 64))
            val nonce = s("echoNonce").toByteArray(Charsets.US_ASCII)
            require(DebugPcEchoCodec.sha(nonce) == s("echoNonceSha256"))
            return DebugPcEchoTarget(m.filterKeys { it != "echoNonce" }, nonce)
        }
    }
}

/** One resource in the original session lease; never a second receiver/lifecycle owner. */
internal class DebugPcEchoReady private constructor(val target: DebugPcEchoTarget, val deadlineElapsed: Long,
                                                   val requestId: String, val remainingRequests: Int) : Closeable {
    @Volatile private var erased = false
    fun current(now: Long) = !erased && now >= 0 && now < deadlineElapsed
    @Synchronized fun erase() { erased = true; target.erase() }
    override fun close() = erase()
    override fun toString() = "DebugPcEchoReady(<private>)"
    companion object {
        /** Only the HMAC-authenticated original root driver response can reach this issuer. */
        fun fromReply(reply: Map<String, Any?>, requestId: String, anchor: Long, now: Long,
                      sessionDeadline: Long, apk: String): DebugPcEchoReady {
            check(BuildConfig.DEBUG) { "Debug PC echo is disabled" }
            val m = DebugPcEchoCodec.objectValue(reply, setOf("schema", "requestId", "action", "target",
                "remainingLifetimeMs", "remainingRequests", "originalReady", "originalHandoff"))
            require(m["schema"] == "polaris-debug-pc-echo-response-v1" && m["requestId"] == requestId && m["action"] == "ready")
            val t = DebugPcEchoTarget.decode(m["target"])
            try {
                require(t.apkSha == apk && now >= anchor && now - anchor <= 3000)
                val ready = Base64.getDecoder().decode(DebugPcEchoCodec.string(m["originalReady"]))
                require(DebugPcEchoCodec.sha(ready) == t.fields["readyReceiptSha256"])
                val original = DebugPcEchoCodec.parse(ready)
                val handoff = DebugPcEchoCodec.parse(Base64.getDecoder().decode(DebugPcEchoCodec.string(m["originalHandoff"])))
                require(DebugPcEchoCodec.canonical(original).contentEquals(ready))
                require(original["phase"] == "Ready" && original["sourceKind"] == "OperatorDeclared" && handoff["phase"] == "ReadyHandoff")
                for (k in listOf("binding", "pcPlanSha256", "controllerInstanceId", "receiverInstanceId", "startedMonotonicNs", "expiresMonotonicNs"))
                    require(original[k] == handoff[k])
                require(handoff["requestId"] == requestId && handoff["readyReceiptSha256"] == t.fields["readyReceiptSha256"] &&
                    original["pcPlanSha256"] == t.fields["pcPlanSha256"] && original["receiverInstanceId"] == t.fields["receiverInstanceId"])
                require(handoff["tcpSocketInstanceId"] == t.socketId("tcp") && handoff["udpSocketInstanceId"] == t.socketId("udp"))
                val remaining = DebugPcEchoCodec.integer(m["remainingLifetimeMs"], 1, 120000)
                require(remaining <= DebugPcEchoCodec.integer(handoff["remainingLifetimeMs"], 1, 120000))
                val budget = DebugPcEchoCodec.integer(m["remainingRequests"], 1, 32)
                require(budget == handoff["remainingRequests"] && budget >= 14)
                // Anchor predates the request. Deduct the entire local roundtrip; never now+120s.
                val deadline = minOf(sessionDeadline, anchor + remaining)
                require(deadline > now)
                return DebugPcEchoReady(t, deadline, requestId, budget.toInt())
            } catch (failure: Throwable) { t.erase(); throw failure }
        }
    }
}

/** Fixed app-private run-as channel. SDK/files/IPC never execute under a metadata gate. */
internal class DebugPcEchoChannel(private val directory: File, private val secret: ByteArray,
    private val scope: DebugCoreProbeLoan.Scope, private val lease: DebugBatchLease,
    private val ticket: DebugBatchLease.CommandTicket, private val allowed: () -> Boolean,
    private val rootCurrent: () -> Unit = {}, initialSequence: Int = 0) {
    init { check(BuildConfig.DEBUG) { "Debug PC echo is disabled" } }
    private var sequence = initialSequence
    private val random = SecureRandom()
    fun exchange(action: String, phase: String): Triple<Map<String, Any?>, String, Long> {
        check(BuildConfig.DEBUG) { "Debug PC echo is disabled" }
        require(action in setOf("ready", "snapshot") && ++sequence <= 25 && phase.length <= 64)
        check(allowed() && lease.ownsProbe(ticket))
        val id = DebugBatchProtocol.hex(ByteArray(16).also(random::nextBytes))
        val request = mapOf("schema" to "polaris-debug-pc-echo-request-v1", "requestId" to id,
            "action" to action, "phase" to phase, "sequence" to sequence,
            "bootNonce" to scope.bootNonce, "sessionId" to scope.sessionId, "nonce" to scope.nonce,
            "planSha256" to scope.planSha256, "apkSha256" to scope.apkSha256, "expectedSourcePin" to scope.expectedSourcePin)
        check(directory.mkdirs() || directory.isDirectory)
        val requestFile = File(directory, "request.json"); val replyFile = File(directory, "reply.json")
        val temporary = File(directory, "request.tmp")
        replyFile.delete()
        val anchor = SystemClock.elapsedRealtime() // BEFORE any request write, not receipt-time renewal.
        val bytes = DebugPcEchoCodec.envelope(secret, request)
        check(lease.beginAcquire())
        val stream = try { temporary.outputStream() } catch (failure: Throwable) { lease.acquireFailed(); throw failure }
        check(lease.publish(stream))
        try {
            check(allowed() && lease.ownsProbe(ticket)); stream.write(bytes); stream.flush()
        } finally { bytes.fill(0); lease.retire(stream) }
        check(temporary.renameTo(requestFile))
        try {
            while (SystemClock.elapsedRealtime() - anchor < 3000) {
                check(allowed() && lease.ownsProbe(ticket))
                if (replyFile.isFile) {
                    require(replyFile.length() in 2..DebugPcEchoCodec.LIMIT.toLong())
                    check(lease.beginAcquire())
                    val input = try { replyFile.inputStream() } catch (failure: Throwable) { lease.acquireFailed(); throw failure }
                    check(lease.publish(input))
                    val raw = try {
                        val out = java.io.ByteArrayOutputStream(); val buffer = ByteArray(4096)
                        while (out.size() <= DebugPcEchoCodec.LIMIT) {
                            val n = input.read(buffer, 0, minOf(buffer.size, DebugPcEchoCodec.LIMIT + 1 - out.size()))
                            if (n < 0) break; check(n > 0); out.write(buffer, 0, n)
                        }
                        out.toByteArray().also { require(it.size <= DebugPcEchoCodec.LIMIT) }
                    } finally { lease.retire(input) }
                    try {
                        val reply = DebugPcEchoCodec.authenticated(secret, raw)
                        check(reply["requestId"] == id && reply["action"] == action && allowed() && lease.ownsProbe(ticket))
                        rootCurrent() // original controller-liveness budget; never renew PC/session TTL
                        return Triple(reply, id, anchor)
                    } finally { raw.fill(0) }
                }
                Thread.sleep(20)
            }
            error("Current PC Ready unavailable")
        } finally { requestFile.delete(); replyFile.delete(); temporary.delete(); directory.delete() }
    }
    fun ready(): DebugPcEchoReady {
        val (reply, id, anchor) = exchange("ready", "ready")
        val value = DebugPcEchoReady.fromReply(reply, id, anchor, SystemClock.elapsedRealtime(), scope.deadlineElapsed, scope.apkSha256)
        check(lease.beginAcquire()) { value.erase(); "Original session sealed" }
        check(lease.publish(value))
        return value
    }
    fun snapshot(phase: String): Map<String, Any?> {
        val (reply, id, _) = exchange("snapshot", phase)
        val m = DebugPcEchoCodec.objectValue(reply, setOf("schema", "requestId", "action", "originalCounters"))
        require(m["schema"] == "polaris-debug-pc-echo-response-v1" && m["requestId"] == id)
        return DebugPcEchoCodec.parse(Base64.getDecoder().decode(DebugPcEchoCodec.string(m["originalCounters"])))
    }
    companion object { fun directory(activity: Activity) = File(activity.filesDir, "debug-pc-echo-v1") }
}
