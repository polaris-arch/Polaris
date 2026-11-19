package com.polaris2.app.vpn

import androidx.test.platform.app.InstrumentationRegistry
import app.tauri.plugin.Invoke
import com.fasterxml.jackson.databind.ObjectMapper
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Runs against Android's real org.json implementation, not the local JVM android.jar stubs. */
class SystemEndpointGuardTest {
    @Test fun replacementAfterExactCloseUsesFreshScopeAndRejectsOldCallbacks() {
        val config = """{"outbounds":[{"type":"direct","tag":"app"}]}"""
        val firstTicket = AndroidNativeAdmission.Ticket("fixture-1", AndroidNativeAdmission.Kind.Main, "app-attempt")
        val retryTicket = AndroidNativeAdmission.Ticket("fixture-2", AndroidNativeAdmission.Kind.Main, "app-retry")
        val first = MainStartRequest(config, "app-attempt", SystemStart.sha256(config.toByteArray()), null, firstTicket)
        val retry = first.copy(runId = "app-retry", nativeTicket = retryTicket)
        val old = MainKernelAttempt<Any>(runId = first.runId, nativeTicket = firstTicket)
        val successor = MainKernelAttempt<Any>(runId = retry.runId, nativeTicket = retryTicket)
        val replies = mutableListOf<Long>()
        val invoke = Invoke(0L, "start", 1L, 2L, { callback, _ -> replies.add(callback) }, "{}", ObjectMapper())
        try {
            assertTrue(VpnBridge.reserveStart(first.runId) { firstTicket })
            assertTrue(VpnBridge.beginStart(first, invoke))
            VpnBridge.releaseStart(first.runId)
            assertEquals(first, VpnBridge.currentStartRequest())
            assertTrue(VpnBridge.finishStart(null, attempt = old))
            assertFalse(VpnBridge.reserveStart(retry.runId) { retryTicket })
            VpnBridge.finishStop(attempt = old)
            assertTrue(VpnBridge.reserveStart(retry.runId) { retryTicket })
            VpnBridge.releaseStart(first.runId)
            assertEquals(retryTicket, VpnBridge.reservedTicket(retry.runId))
            assertTrue(VpnBridge.beginStart(retry, invoke))
            assertFalse(VpnBridge.finishStart(null, attempt = old))
            VpnBridge.finishStop(attempt = old)
            assertEquals(retry, VpnBridge.currentStartRequest())
            assertTrue(VpnBridge.finishStart(null, attempt = successor))
            assertEquals(listOf(1L, 1L), replies)
        } finally {
            VpnBridge.releaseStart(first.runId)
            VpnBridge.releaseStart(retry.runId)
            VpnBridge.finishStop(attempt = old)
            VpnBridge.finishStop(attempt = successor)
        }
    }

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
