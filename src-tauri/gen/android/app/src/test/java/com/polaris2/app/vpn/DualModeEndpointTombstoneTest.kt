package com.polaris2.app.vpn

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.fail
import org.junit.Test

class DualModeEndpointTombstoneTest {
    private fun dual(port: Int, mode: String = "normal") = """
        {"experimental":{"clash_api":{"default_mode":"$mode"}},
         "services":[{"type":"api","listen":"127.0.0.1","listen_port":$port,"secret":"redacted"}]}
    """.trimIndent()
    private val ordinary = """{"services":[{"type":"api","listen_port":9091}]}"""

    private inline fun <reified T : Throwable> rejects(action: () -> Unit) {
        try { action(); fail("expected ${T::class.java.simpleName}") }
        catch (error: Throwable) { if (error !is T) throw error }
    }

    @Test fun destroyThenSystemStartCannotReuseOldDualEndpoint() {
        val process = DualModeEndpointTombstone()
        assertEquals(18001, process.claimBirth(dual(18001)))
        // The BoxService and its attempt may be gone; process tombstone remains.
        rejects<DualModeEndpointTombstone.Retired> { process.requireFreshBridgeEndpoint(dual(18001)) }
        rejects<DualModeEndpointTombstone.Retired> { process.claimBirth(dual(18001)) }
    }

    @Test fun failedBirthRetiresEndpointButBridgeFreshPortCanStart() {
        val process = DualModeEndpointTombstone()
        process.claimBirth(dual(18001, "mesh-direct")) // native birth subsequently fails
        rejects<DualModeEndpointTombstone.Retired> { process.claimBirth(dual(18001)) }
        assertEquals(18002, process.claimBirth(dual(18002)))
        process.requireFreshBridgeEndpoint(dual(18003))
    }

    @Test fun dualModeReloadIsRefusedBeforeNativeAndLeavesActiveEndpointClaimed() {
        val process = DualModeEndpointTombstone()
        process.claimBirth(dual(18001))
        rejects<DualModeEndpointTombstone.ReloadRequiresReconnect> {
            process.requireReloadAllowed(true, dual(18001, "mesh-direct"))
        }
        rejects<DualModeEndpointTombstone.ReloadRequiresReconnect> {
            process.requireReloadAllowed(true, ordinary)
        }
        rejects<DualModeEndpointTombstone.Retired> { process.claimBirth(dual(18001)) }
    }

    @Test fun ordinaryReloadAndNewProcessFirstAlwaysOnRetainExistingBehavior() {
        val process = DualModeEndpointTombstone()
        assertNull(process.claimBirth(ordinary))
        process.requireReloadAllowed(false, ordinary)
        rejects<DualModeEndpointTombstone.ReloadRequiresReconnect> {
            process.requireReloadAllowed(false, dual(18001))
        }
        val freshProcess = DualModeEndpointTombstone()
        assertEquals(18001, freshProcess.claimBirth(dual(18001)))
    }

    @Test fun malformedDualEndpointFailsClosedWithoutLeakingConfig() {
        val process = DualModeEndpointTombstone()
        rejects<DualModeEndpointTombstone.Invalid> {
            process.claimBirth(dual(0))
        }
        rejects<DualModeEndpointTombstone.Invalid> {
            process.claimBirth(dual(18001, "unexpected"))
        }
    }
}
