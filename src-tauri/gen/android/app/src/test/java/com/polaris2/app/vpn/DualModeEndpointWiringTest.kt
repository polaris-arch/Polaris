package com.polaris2.app.vpn

import java.io.File
import org.junit.Assert.assertTrue
import org.junit.Test

/** Source-order gates for Android Service calls that local JVM tests cannot instantiate. */
class DualModeEndpointWiringTest {
    private val service = File("src/main/java/com/polaris2/app/vpn/BoxService.kt").readText()
    private val plugin = File("src/main/java/com/polaris2/app/vpn/PolarisVpnPlugin.kt").readText()

    @Test fun nativeBirthClaimsEndpointBeforeCreatingOrStartingServer() {
        val start = service.substringAfter("private fun startKernel(").substringBefore("// ── CommandServerHandler")
        val claim = start.indexOf("MainDualModeEndpointTombstone.claimBirth(config)")
        val create = start.indexOf("Libbox.newStrictCommandServer(")
        val nativeStart = start.indexOf("server.startOrReloadService(config, OverrideOptions())")
        assertTrue(claim >= 0 && claim < create && create < nativeStart)
    }

    @Test fun dualReloadRefusalPrecedesNativeReloadAndKeepsLiveCore() {
        val reload = service.substringAfter("private fun serviceReload(").substringBefore("private fun setReloadError(")
        val guard = reload.indexOf("requireReloadAllowed(attempt.dualModeApiPort != null, config)")
        val nativeReload = reload.indexOf("server.startOrReloadService(config, OverrideOptions())")
        assertTrue(guard >= 0 && guard < nativeReload)
        assertTrue(reload.contains("catch (_: DualModeEndpointTombstone.ReloadRequiresReconnect)"))
        assertTrue(reload.contains("showReconnectNotice(attempt)"))
    }

    @Test fun typedNoBirthIsOnlyEmittedBeforeBridgeAdmission() {
        val start = plugin.substringAfter("fun start(invoke: Invoke)").substringBefore("/**\n     * 停核")
        val preflight = start.indexOf("requireFreshBridgeEndpoint(cfg)")
        val typed = start.indexOf("invoke.reject(error.message, ERR_ENDPOINT_RETIRED)")
        val admission = start.indexOf("VpnBridge.beginStart(")
        assertTrue(preflight >= 0 && typed > preflight && typed < admission)
        assertTrue(service.contains("NativeReconnectNotice.require(service, attempt.birthNonce)"))
        assertTrue(service.contains("runCatching { showReconnectNotice(attempt) }"))
    }

    @Test fun rejectedSystemStartKeepsOwnedReasonUntilExplicitRecovery() {
        val notice = service.substringAfter("private fun showReconnectNotice(").substringBefore("// Android 没有")
        val marker = notice.indexOf("NativeReconnectNotice.require(service, attempt.birthNonce)")
        val notification = notice.indexOf("PolarisApplication.notification.notify(")
        assertTrue(marker >= 0 && marker < notification)
        val start = service.substringAfter("private fun startKernel(").substringBefore("// ── CommandServerHandler")
        assertTrue(start.contains("runCatching { showReconnectNotice(attempt) }"))
        assertTrue(start.contains("if (current) stopService(attempt)"))
        assertTrue(start.contains("if (bridgeConfig != null) NativeReconnectNotice.clear(service)"))
        val closed = service.substringAfter("private fun onAttemptClosed(").substringBefore("override fun serviceStart")
        val completion = closed.indexOf("MainKernelAttemptRegistry.completeAfterClose(attempt)")
        val guard = closed.indexOf("if (attempt.clearReconnectNoticeOnClose)")
        val clear = closed.indexOf("NativeReconnectNotice.clearIfOwner(service, attempt.birthNonce)")
        assertTrue(completion >= 0 && completion < guard)
        assertTrue(guard < clear)
        assertTrue(completion >= 0 && completion < clear)
        assertTrue(closed.substring(0, completion).contains("if (failure != null)"))
        assertTrue(service.contains("stopService(userRequested = true)"))
        val stopping = service.substringAfter("private fun stopService(").substringBefore("private fun onAttemptClosed(")
        val explicit = stopping.indexOf("if (userRequested && attempt != null) attempt.clearReconnectNoticeOnClose = true")
        val inFlight = stopping.indexOf("if (state == ServiceState.Stopping && !closeFailed) return")
        assertTrue(explicit >= 0 && explicit < inFlight)
        val alreadyStopped = plugin.substringAfter("VpnBridge.StopAdmission.AlreadyStopped ->")
            .substringBefore("VpnBridge.StopAdmission.Busy ->")
        assertTrue(alreadyStopped.contains("MainKernelAttemptRegistry.isVacant()"))
        assertTrue(alreadyStopped.contains("NativeReconnectNotice.clearIfOwner(activity, it)"))
    }
}
