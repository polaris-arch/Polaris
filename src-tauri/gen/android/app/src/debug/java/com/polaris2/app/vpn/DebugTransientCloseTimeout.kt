package com.polaris2.app.vpn

import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicLong

/** ACC-01: retain one real closeService until this App process dies. No reset/release operation. */
internal class DebugTransientCloseTimeout {
    companion object {
        val process = DebugTransientCloseTimeout()
        private val nextToken = AtomicLong(1)

        private fun validRequest(kind: String, id: String): Boolean =
            kind in setOf("speedtest", "login") && id.length in 1..256 &&
                id.all { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it in "._:-" }
    }

    private val lock = Any()
    private val running = mutableMapOf<Pair<String, String>, Target>()
    private var armed: Target? = null

    /** Read-only snapshot; arm rechecks the current engine under the same lock. */
    fun currentTargets(): List<Triple<String, String, String>> = synchronized(lock) {
        running.map { (key, target) -> Triple(key.first, key.second, target.token) }
            .sortedWith(compareBy({ it.first }, { it.second }))
    }

    /** A fresh object belongs to one LibboxEngine, even if a caller later repeats its logical ID. */
    inner class Target(private val kind: String, private val id: String) {
        // Diagnostic identity only; checked process counter cannot wrap or address a successor.
        val token = nextToken.getAndUpdate { Math.incrementExact(it) }.toString(16).padStart(16, '0')
        private val key = kind to id
        private var closing = false
        private val processLifetime = CountDownLatch(1)

        init { require(validRequest(kind, id)) { "Invalid transient close timeout target" } }

        fun started() = synchronized(lock) {
            // Native Start can return after Close has already entered this exact engine.
            if (!closing) {
                check(running[key] == null || running[key] === this) { "Transient close timeout target already running" }
                check(running.size < 9 || running[key] === this) { "Transient close timeout target limit reached" }
                running[key] = this
            }
        }

        /** Called inside production cleanup, after resolver fencing and before native closeService. */
        fun beforeClose() {
            val block = synchronized(lock) {
                closing = true
                if (running[key] === this) running.remove(key)
                armed === this
            }
            if (block) {
                // Interruption cannot fabricate a failed/successful native close or clear ownership.
                while (true) {
                    try { processLifetime.await() }
                    catch (_: InterruptedException) { continue }
                }
            }
        }
    }

    fun arm(kind: String, id: String, token: String) = synchronized(lock) {
        require(validRequest(kind, id)) { "Invalid transient close timeout target" }
        require(token.length == 16 && token.all { it in '0'..'9' || it in 'a'..'f' }) { "Invalid transient close timeout token" }
        check(armed == null) { "Transient close timeout already armed; restart the App process" }
        val target = checkNotNull(running[kind to id]) { "Transient close timeout target is not running" }
        check(target.token == token) { "Transient close timeout target is no longer current" }
        armed = target
    }
}
