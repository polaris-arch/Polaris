package com.polaris2.app.vpn

import java.io.File
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

/** Pure protojson fixtures and source wiring; no libbox, JNI, VPN or Android service is run. */
class RuntimeProtocolAdmissionTest {
    private val methods = listOf("GetRuntimeCapabilities", "PlanRuntimeConfig", "ApplyRuntimeConfig",
        "GetRuntimeConfigStatus", "RetryRuntimePersistence", "AcknowledgeRuntimePersistence")
    private val digest = "a".repeat(64)
    private fun fixture() = JSONObject().put("contract_version", 1).put("connected_hot_enabled", true)
        .put("implementation_manifest_digest", digest)
        .put("active", JSONObject().put("incarnation", "original-instance")
            .put("applied_revision", "1").put("effective_digest", digest))
        .put("fields", JSONArray().put(JSONObject().put("effective_path", "runtime.protocol")
            .put("variant", "polaris-daily-runtime-v1").put("adapter_version", "1")
            .put("platform_build", "android/arm64").put("conditions", JSONArray(methods))))
    private fun reason(value: JSONObject, code: String, path: String = "runtime") = value.apply {
        getJSONArray("fields").put(JSONObject().put("effective_path", "runtime.instance")
            .put("variant", code).put("conditions", JSONArray(listOf(path))))
    }
    private fun decode(value: JSONObject) = RuntimeProtocolAdmission.decode(value.toString(), "android/arm64")
    private fun rejected(value: JSONObject) { assertThrows(Exception::class.java) { decode(value) } }
    private fun unsupported(): JSONObject = reason(reason(fixture().apply {
        remove("connected_hot_enabled"); remove("active")
    }, "RUNTIME_STARTED_UNSUPPORTED"), "PENDING_ADAPTER", "services")

    @Test fun defaultOmissionsDoNotDisableTheRealHotGraph() {
        val admission = decode(fixture())
        assertEquals("original-instance", admission.identity?.incarnation)
        assertEquals("0", admission.controlRevision)
        assertTrue(admission.unsupportedPaths.isEmpty())
    }

    @Test fun missingWrongOrDuplicateProtocolNeverAdmits() {
        rejected(fixture().put("fields", JSONArray()))
        rejected(fixture().put("contract_version", 2))
        for ((key, value) in listOf("variant" to "upstream", "adapter_version" to "2",
            "platform_build" to "linux/arm64", "update_class" to "IN_PLACE")) {
            rejected(fixture().apply { getJSONArray("fields").getJSONObject(0).put(key, value) })
        }
        rejected(fixture().apply { val fields = getJSONArray("fields"); fields.put(fields.getJSONObject(0)) })
        rejected(fixture().apply { getJSONArray("fields").getJSONObject(0).put("conditions", JSONArray(methods.reversed())) })
        rejected(fixture().apply { getJSONArray("fields").getJSONObject(0).put("conditions", JSONArray(methods.dropLast(1))) })
    }

    @Test fun noDigestOrNonHexManifestIsNotObservedProtocolEvidence() {
        for (value in listOf("", "a".repeat(63), "g".repeat(64), "A".repeat(64))) {
            rejected(fixture().put("implementation_manifest_digest", value))
        }
        rejected(fixture().apply { remove("implementation_manifest_digest") })
    }

    @Test fun uint64StringsRemainExactBeyondFloatAndSignedLongRanges() {
        for (value in listOf("9007199254740993", "18446744073709551615")) {
            val source = fixture().put("control_revision", value)
            source.getJSONObject("active").put("applied_revision", value)
            val admission = decode(source)
            assertEquals(value, admission.controlRevision)
            assertEquals(value, admission.identity?.appliedRevision)
        }
        for (value in listOf<Any>("18446744073709551616", "01", "-1", "1.0", "1e3", 1, 1.0, JSONObject.NULL)) {
            rejected(fixture().put("control_revision", value))
            rejected(fixture().apply { getJSONObject("active").put("applied_revision", value) })
        }
    }

    @Test fun hotTrueNeedsACompleteNonzeroIdentity() {
        rejected(fixture().apply { remove("active") })
        for ((key, value) in listOf("incarnation" to "", "applied_revision" to "0", "effective_digest" to "")) {
            rejected(fixture().apply { getJSONObject("active").put(key, value) })
        }
        rejected(fixture().put("connected_hot_enabled", "true"))
    }

    @Test fun disabledAndPendingAdapterAloneAreNeverReady() {
        rejected(fixture().put("connected_hot_enabled", false))
        rejected(fixture().apply { remove("connected_hot_enabled") })
        rejected(reason(fixture().apply { remove("connected_hot_enabled"); remove("active") }, "PENDING_ADAPTER", "services"))
        rejected(reason(fixture().apply { remove("connected_hot_enabled"); remove("active") }, "RUNTIME_STARTED_UNSUPPORTED"))
    }

    @Test fun explicitStartedUnsupportedRetainsItsConcreteGraphReason() {
        val admission = decode(unsupported())
        assertNull(admission.identity)
        assertEquals(listOf("services"), admission.unsupportedPaths)
        rejected(unsupported().put("connected_hot_enabled", true))
        rejected(unsupported().put("active", fixture().getJSONObject("active")))
        rejected(reason(unsupported(), "RUNTIME_STARTED_UNSUPPORTED"))
    }

    @Test fun stoppedStartingUnavailableOrUnknownNeverReadyEvenWithOtherGoodFields() {
        for (code in listOf("RUNTIME_STARTING", "RUNTIME_STOPPED", "RUNTIME_GRAPH_UNAVAILABLE",
            "INITIAL_CONFIG_ENCODING_FAILED", "INVALID_ACTIVE_SNAPSHOT", "UNKNOWN_REASON")) {
            rejected(reason(fixture(), code))
            rejected(reason(unsupported(), code))
        }
        rejected(reason(fixture(), "PENDING_ADAPTER", "services"))
    }

    @Test fun persistenceWarningAndUnknownBuildTagsAreNotGlobalOffSwitches() {
        val value = reason(fixture(), "OWNER_PERSISTENCE_UNBOUND", "runtime.persistence")
        value.getJSONArray("fields").put(JSONObject().put("effective_path", "runtime.build")
            .put("variant", "with_naive_outbound").put("conditions", JSONArray(listOf("state=unknown"))))
        assertNotNull(decode(value).identity)
        assertNull(decode(reason(unsupported(), "OWNER_PERSISTENCE_UNBOUND", "runtime.persistence")).identity)
        rejected(reason(fixture(), "OWNER_PERSISTENCE_UNBOUND", "wrong.path"))
    }

    @Test fun targetComesFromAppBuildAndProcessNotTheCoreClaim() {
        assertEquals("android/arm64", RuntimeProtocolAdmission.androidPlatform("arm64", true, emptyList()))
        assertEquals("android/arm", RuntimeProtocolAdmission.androidPlatform("arm", false, emptyList()))
        assertEquals("android/386", RuntimeProtocolAdmission.androidPlatform("universal", false, listOf("x86")))
        assertEquals("android/amd64", RuntimeProtocolAdmission.androidPlatform("x86_64", true, emptyList()))
        assertThrows(Exception::class.java) { RuntimeProtocolAdmission.androidPlatform("arm", true, emptyList()) }
        assertThrows(Exception::class.java) { RuntimeProtocolAdmission.androidPlatform("unknown", true, listOf("arm64-v8a")) }
        assertThrows(Exception::class.java) { RuntimeProtocolAdmission.androidPlatform("universal", true, emptyList()) }
    }

    @Test fun everyNativeBirthQueriesBeforeReadinessAndFailureClosesOnlyItsAttempt() {
        val box = File("src/main/java/com/polaris2/app/vpn/BoxService.kt").readText()
        val start = box.substringAfter("private fun startKernel(").substringBefore("// ── CommandServerHandler")
        val birth = start.indexOf("server.startOrReloadServiceWithRetryableRuntimePersistence(")
        val admission = start.indexOf("admitRuntimeProtocol(attempt, server)")
        val ready = start.indexOf("state = ServiceState.Started")
        assertTrue(birth >= 0 && birth < admission && admission < ready)
        assertTrue(ready < start.indexOf("VpnBridge.finishStart(null"))
        assertTrue(start.contains("check(runtimeAdmission != null)"))
        assertTrue(start.contains("if (current) stopService(attempt)"))
        val reload = box.substringAfter("nativeReload = { server, config ->").substringBefore("setError =")
        assertTrue(reload.indexOf("startOrReloadServiceWithRetryableRuntimePersistence(") < reload.indexOf("admitRuntimeProtocol(attempt, server)"))
        assertTrue(reload.contains("stopService(attempt)"))
        val gate = box.substringAfter("private fun admitRuntimeProtocol(").substringBefore("/** New native incarnation")
        for (guard in listOf("Thread.holdsLock(attempt.operationLock)", "mainAttempt === attempt",
            "commandServer === server", "runtimePersistenceEpoch == epoch", "!attempt.revoked",
            "MainKernelAttemptRegistry.isCurrent(attempt)", "server.runtimeCapabilities()")) assertTrue(guard, gate.contains(guard))
        assertFalse(gate.contains("BuildConfig.DEBUG"))
        assertFalse(gate.contains("stopService()"))
        assertTrue(box.contains("runtimeAdmission?.identity?.incarnation == runtimeIncarnation"))
    }
}
