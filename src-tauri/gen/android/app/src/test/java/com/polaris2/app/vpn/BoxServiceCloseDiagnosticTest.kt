package com.polaris2.app.vpn

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class BoxServiceCloseDiagnosticTest {
    @Test fun nativeFailureNamesComponentsWithoutLeakingNodeDetails() {
        val message = "close endpoint/tailscale[private-tag]: secret-token; close network: 198.51.100.1"
        val diagnostic = safeCloseFailureComponent(IllegalStateException(message))
        assertEquals("endpoint+network", diagnostic)
        assertFalse(diagnostic.contains("private-tag"))
        assertFalse(diagnostic.contains("secret-token"))
        assertFalse(diagnostic.contains("198.51.100.1"))
    }

    @Test fun unknownFailureStillHasAStableLabel() {
        assertEquals("unknown", safeCloseFailureComponent(IllegalStateException("sensitive config")))
    }
}
