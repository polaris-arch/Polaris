package com.polaris2.app.vpn

import com.polaris2.app.BuildConfig
import org.junit.Assert.*
import org.junit.Assume.assumeFalse
import org.junit.Test

class DebugCoreProbeReleaseGuardTest {
    @Test fun releasePrivateLoanAndScopeRejectBeforeMainReadsOrExecutor() {
        assumeFalse(BuildConfig.DEBUG)
        val args = DebugCoreProbeLoanArgs().also { it.password = "b".repeat(32).toByteArray() }
        val failures = listOf(
            runCatching { DebugCoreProbeLoan.currentInput() },
            runCatching { DebugBatchQa.coreProbeScopeTask("2".repeat(32), {}, { throw it }) },
            runCatching { DebugBatchQa.coreProbeLoanTask(args, {}, { throw it }) },
        )
        failures.forEach { assertEquals("Debug core probe loan is disabled", it.exceptionOrNull()?.message) }
        assertTrue(args.password.all { it == 0.toByte() })
        val executor = DebugBatchCommandExecutor.javaClass.getDeclaredField("value\$delegate").also { it.isAccessible = true }
        assertFalse((executor.get(null) as Lazy<*>).isInitialized())
    }
}
