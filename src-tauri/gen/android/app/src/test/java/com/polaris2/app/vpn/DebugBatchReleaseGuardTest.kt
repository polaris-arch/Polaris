package com.polaris2.app.vpn

import android.app.Activity
import com.polaris2.app.BuildConfig
import org.junit.Assert.*
import org.junit.Assume.assumeFalse
import org.junit.Test

class DebugBatchReleaseGuardTest {
    @Test fun directReleaseManagerCallRejectsBeforeAnySdkOrSessionExecutor() {
        assumeFalse(BuildConfig.DEBUG)
        // An uninitialized SDK object deliberately cannot supply any Context API.
        val unsafe = Class.forName("sun.misc.Unsafe")
        val field = unsafe.getDeclaredField("theUnsafe").also { it.isAccessible = true }
        val activity = unsafe.getMethod("allocateInstance", Class::class.java).invoke(field.get(null), Activity::class.java) as Activity
        val result = runCatching { DebugBatchQa.command(activity, "prepare", null, "{}") }
        assertTrue(result.exceptionOrNull() is IllegalStateException)
        assertEquals("Debug batch QA is disabled", result.exceptionOrNull()?.message)
        val active = DebugBatchQa.javaClass.getDeclaredField("active").also { it.isAccessible = true }
        assertNull(active.get(null))
        val executor = DebugBatchCommandExecutor.javaClass.getDeclaredField("value\$delegate").also { it.isAccessible = true }
        assertFalse((executor.get(null) as Lazy<*>).isInitialized())
    }
}
