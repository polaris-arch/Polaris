package com.polaris2.app.vpn

import org.junit.Assert.*
import org.junit.Test

class DebugTransientCloseTimeoutReleaseTest {
    @Test fun releaseCannotArmOrRetainAnyFaultState() {
        val injection = DebugTransientCloseTimeout.process
        val result = runCatching { injection.arm("login", "tailscale-login-fixture-1", "0".repeat(16)) }
        assertEquals("Debug transient close timeout is disabled", result.exceptionOrNull()?.message)
        assertEquals("Debug transient close timeout is disabled", runCatching { injection.currentTargets() }.exceptionOrNull()?.message)
        val target = injection.Target("login", "tailscale-login-fixture-1")
        target.started(); target.beforeClose()
        assertTrue(injection.javaClass.declaredFields.all { java.lang.reflect.Modifier.isStatic(it.modifiers) })
        assertTrue(target.javaClass.declaredFields.all { it.type == injection.javaClass })
    }
}
