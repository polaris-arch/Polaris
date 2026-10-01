package com.polaris2.app.vpn

import java.security.MessageDigest
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/** Fixed LAN witness wire. Secrets/payloads are never included in snapshots. */
internal object DebugBatchProtocol {
    const val SCHEMA = "polaris-android-lan-v2"
    const val MAX_BYTES = 512
    const val MAX_BUDGET = 256 * 1024
    fun hex(bytes: ByteArray) = bytes.joinToString("") { "%02x".format(it.toInt() and 255) }
    fun unhex(value: String): ByteArray {
        require(value.matches(Regex("[0-9a-f]{64}"))) { "invalid session secret" }
        return value.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
    }
    fun sha(value: String) = hex(MessageDigest.getInstance("SHA-256").digest(value.toByteArray(Charsets.UTF_8)))
    fun literalLanIPv4(value: String): Boolean {
        val parts = value.split('.')
        if (parts.size != 4 || parts.any { !it.matches(Regex("0|[1-9][0-9]{0,2}")) || it.toInt() > 255 }) return false
        val a = parts.map(String::toInt)
        return a[3] in 1..254 && (a[0] == 10 || (a[0] == 172 && a[1] in 16..31) || (a[0] == 192 && a[1] == 168))
    }
    fun validatePlan(peers: List<String>, port: Int, ttlMillis: Long) {
        require(peers.size in 1..2 && peers.distinct().size == peers.size && peers.all(::literalLanIPv4)) { "literal approved LAN peers required" }
        require(port in 47100..47115 && ttlMillis in 1000..300000) { "plan outside finite profile" }
    }
    data class Frame(val kind: String, val planSha: String, val sessionId: String, val nonce: String,
                     val instanceId: String, val role: String, val protocol: String, val sequence: Long,
                     val challenge: String, val deadline: Long) {
        fun canonical() = listOf("2", kind, planSha, sessionId, nonce, instanceId, role,
                                "reachability", "peer-to-app", protocol, sequence.toString(), challenge, deadline.toString()).joinToString("|")
    }
    private fun mac(secret: ByteArray, value: ByteArray): ByteArray = Mac.getInstance("HmacSHA256").run {
        init(SecretKeySpec(secret, "HmacSHA256")); doFinal(value)
    }
    private fun key(secret: ByteArray, frame: Frame) = mac(secret, "polaris-lan-v2/key/${frame.role}/peer-to-app/${frame.protocol}".toByteArray(Charsets.US_ASCII))
    fun encode(secret: ByteArray, frame: Frame): ByteArray {
        val body = frame.canonical()
        return (body + "|" + hex(mac(key(secret, frame), body.toByteArray(Charsets.US_ASCII)))).toByteArray(Charsets.US_ASCII).also { require(it.size <= MAX_BYTES) }
    }
    fun decode(secret: ByteArray, bytes: ByteArray): Frame? = runCatching {
        require(bytes.size in 1..MAX_BYTES && bytes.all { it.toInt() in 32..126 })
        val parts = bytes.toString(Charsets.US_ASCII).split('|')
        require(parts.size == 14 && parts[0] == "2" && parts[7] == "reachability" && parts[8] == "peer-to-app")
        require(parts[1] in listOf("REQ", "ACK") && parts[6] in listOf("data", "health") && parts[9] in listOf("tcp", "udp"))
        require(parts[2].matches(Regex("[0-9a-f]{64}")) && parts[3].matches(Regex("[0-9a-f]{32}")) &&
                parts[4].matches(Regex("[0-9a-f]{48}")) && parts[5].matches(Regex("[0-9a-f]{32}")) && parts[11].matches(Regex("[0-9a-f]{32}")))
        val frame = Frame(parts[1], parts[2], parts[3], parts[4], parts[5], parts[6], parts[9], parts[10].toLong(), parts[11], parts[12].toLong())
        require(frame.sequence in 1..20 && frame.deadline > 0)
        require(MessageDigest.isEqual(encode(secret, frame), bytes))
        frame
    }.getOrNull()
}

/** Bounded nonce ledger, separate health/data replay spaces; seal freezes counters permanently. */
internal class DebugBatchCounter(private val planSha: String, private val sessionId: String,
                                private val nonce: String, private val instance: String,
                                private val protocol: String, private val deadline: Long) {
    data class Snapshot(val matched: Int, val health: Int, val invalid: Int, val replay: Int,
                        val bytes: Int, val sealed: Boolean)
    private val seen = mutableSetOf<Pair<String, Long>>()
    private var value = Snapshot(0, 0, 0, 0, 0, false)
    @Synchronized fun accept(frame: DebugBatchProtocol.Frame?, length: Int, now: Long): Boolean {
        if (value.sealed) return false
        if (frame == null || frame.kind != "REQ" || frame.planSha != planSha || frame.sessionId != sessionId ||
            frame.nonce != nonce || frame.instanceId != instance || frame.protocol != protocol || frame.deadline != deadline || now >= deadline ||
            value.bytes + length > DebugBatchProtocol.MAX_BUDGET) {
            value = value.copy(invalid = minOf(1000, value.invalid + 1)); return false
        }
        if (!seen.add(frame.role to frame.sequence)) { value = value.copy(replay = minOf(1000, value.replay + 1)); return false }
        value = value.copy(matched = value.matched + if (frame.role == "data") 1 else 0,
                           health = value.health + if (frame.role == "health") 1 else 0, bytes = value.bytes + length)
        return true
    }
    @Synchronized fun seal() { value = value.copy(sealed = true) }
    @Synchronized fun snapshot() = value.copy()
}
