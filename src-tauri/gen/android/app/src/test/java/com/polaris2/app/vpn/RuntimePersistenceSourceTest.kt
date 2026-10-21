package com.polaris2.app.vpn

import java.io.File
import org.junit.Assert.*
import org.junit.Test

/** Source wiring guards only; native callback and crash-injection acceptance remain unexecuted. */
class RuntimePersistenceSourceTest {
    private fun source(name: String) = File("src/main/java/com/polaris2/app/vpn/$name.kt").readText()

    @Test fun startupAndReloadCaptureFreshEpochOnTheSameAttempt() {
        val box = source("BoxService")
        assertEquals(2, Regex("server\\.startOrReloadServiceWithRetryableRuntimePersistence\\(").findAll(box).count())
        assertFalse(box.contains("server.startOrReloadService(config, OverrideOptions())"))
        val handler = box.substringAfter("private fun runtimePersistenceHandler").substringBefore("/** Same operationLock")
        for (guard in listOf("mainAttempt === attempt", "commandServer === server", "runtimePersistenceEpoch == epoch",
            "!attempt.revoked", "state == ServiceState.Started", "MainKernelAttemptRegistry.isCurrent(attempt)",
            "incarnation == runtimeIncarnation", "revision >= lastRevision", "lastDigest == effectiveDigest")) {
            assertTrue(guard, handler.contains(guard))
        }
        assertTrue(handler.contains("override fun runtimePersistenceMode(): String = \"exact_configuration_file\""))
        assertFalse(handler.contains("synchronized(attempt.operationLock)"))
        // Repeat-safe entry retains the same revision/digest and owner fences.
        assertFalse(box.contains("server.startOrReloadServiceWithRuntimePersistence("))
        assertTrue(handler.contains("revision != lastRevision || lastDigest == effectiveDigest"))
        assertTrue(box.contains("VpnBridge.currentConfig() ?: bridgeConfig"))
    }

    @Test fun reloadSourceCompareAndEpochClaimShareCallbackMonitor() {
        val box = source("BoxService")
        val reload = box.substringAfter("nativeReload = { server, config ->").substringBefore("setError =")
        assertTrue(reload.indexOf("expectedReloadConfig = config") < reload.indexOf("observeNativeInput("))
        assertTrue(reload.contains("startOrReloadServiceWithRetryableRuntimePersistence(config, OverrideOptions(), persistence)"))
        val claim = box.substringAfter("private fun runtimePersistenceHandler(").substringBefore("return object : RuntimePersistenceHandler")
        val lock = claim.indexOf("synchronized(this)")
        val read = claim.indexOf("VpnBridge.currentConfig() ?: SystemStart.load(service)")
        val compare = claim.indexOf("check(currentConfig == expectedReloadConfig)")
        val epoch = claim.indexOf("++runtimePersistenceEpoch")
        assertTrue(lock >= 0 && lock < read && read < compare && compare < epoch)
        // Native calls remain outside the monitor, avoiding Close/callback deadlock.
        assertFalse(claim.contains("server.startOrReloadService"))
    }

    @Test fun restartSourceUsesFailClosedDigestAndDurableRenames() {
        val system = source("SystemStart")
        val commit = system.substringAfter("fun persistRuntime(").substringBefore("private fun syncDirectory")
        assertTrue(commit.indexOf("invalidate()\n        try") < commit.indexOf("writeDurably(runtimeConfig(context), config)"))
        assertTrue(commit.indexOf("writeDurably(runtimeConfig(context), config)") < commit.indexOf("writeDurably(admission, expectedDigest)"))
        assertTrue(commit.indexOf("writeDurably(admission, expectedDigest)") < commit.indexOf("publish()"))
        assertTrue(commit.contains("disconnectGeneration == expectedGeneration && allowed()"))
        assertTrue(commit.contains("requireLegacyAllowed(context)"))
        assertTrue(commit.contains("try { invalidate() }"))
        assertTrue(system.contains("output.fd.sync()"))
        assertTrue(system.contains("Os.rename(tmp.path, target.path)"))
        assertTrue(system.contains("Os.fsync(fd)"))
    }

    @Test fun bridgePublicationKeepsOriginalRequestIdentityAndRejectsStop() {
        val bridge = source("PolarisVpnPlugin").substringAfter("fun publishRuntimeConfig(")
            .substringBefore("fun currentStartRequest()")
        assertTrue(bridge.contains("running && !starting && !stopping"))
        assertTrue(bridge.contains("current.runId == attempt.runId && current.nativeTicket == attempt.nativeTicket"))
        assertTrue(bridge.contains("current.copy(configContent = config, configDigest = digest)"))
        assertTrue(bridge.contains("check(systemStarted)"))
    }
}
