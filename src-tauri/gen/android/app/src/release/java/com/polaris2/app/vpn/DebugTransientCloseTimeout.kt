package com.polaris2.app.vpn

/** Release has no target registry, latch or fault state. */
internal class DebugTransientCloseTimeout private constructor() {
    companion object { val process = DebugTransientCloseTimeout() }
    @Suppress("UNUSED_PARAMETER")
    inner class Target(kind: String, id: String) {
        fun started() = Unit
        fun beforeClose() = Unit
    }
    @Suppress("UNUSED_PARAMETER")
    fun arm(kind: String, id: String, token: String): Unit = error("Debug transient close timeout is disabled")
    fun currentTargets(): List<Triple<String, String, String>> = error("Debug transient close timeout is disabled")
}
