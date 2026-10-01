package com.polaris2.app.vpn

import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Runs against Android's real org.json implementation, not the local JVM android.jar stubs. */
class SystemEndpointGuardTest {
    @Test fun finalLibboxEndpointCases() {
        val context = InstrumentationRegistry.getInstrumentation().context
        val fixture = context.assets.open("android-system-endpoints.json").bufferedReader().use { it.readText() }
        val cases = JSONObject(fixture).getJSONArray("cases")
        for (index in 0 until cases.length()) {
            val case = cases.getJSONObject(index)
            val name = case.getString("name")
            val failure = runCatching { SystemEndpointGuard.requireSupported(case.getString("config")) }.exceptionOrNull()
            when (case.getString("outcome")) {
                "allowed" -> assertEquals(name, null, failure)
                "unsupported" -> {
                    assertTrue(name, failure is SystemEndpointGuard.Unsupported)
                    assertEquals(name, SystemEndpointGuard.ERROR, failure?.message)
                }
                "invalid" -> {
                    assertTrue(name, failure is IllegalArgumentException)
                    assertFalse(name, failure is SystemEndpointGuard.Unsupported)
                    assertEquals(name, "Invalid libbox endpoint configuration", failure?.message)
                    assertFalse(name, failure?.message.orEmpty().contains("synthetic-secret"))
                }
                else -> error("Unknown fixture outcome: $name")
            }
        }
    }
}
