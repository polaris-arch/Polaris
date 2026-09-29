package com.polaris2.app.vpn

import java.io.File
import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NativeReconnectNoticeTest {
    private inline fun withMarker(test: (File) -> Unit) {
        val dir = Files.createTempDirectory("polaris-native-reconnect-test").toFile()
        try { test(File(dir, "native-reconnect-required")) }
        finally { dir.deleteRecursively() }
    }

    @Test fun automaticRejectedBirthDoesNotRequestNoticeClear() = withMarker { marker ->
        val rejected = MainKernelAttempt<Any>()
        NativeReconnectNotice.require(marker, rejected.birthNonce)
        assertFalse(rejected.clearReconnectNoticeOnClose)
        assertEquals(rejected.birthNonce, NativeReconnectNotice.owner(marker))
    }

    @Test fun oldCloseCannotEraseLaterAttemptNotice() = withMarker { marker ->
        val old = MainKernelAttempt<Any>()
        val successor = MainKernelAttempt<Any>()
        NativeReconnectNotice.require(marker, old.birthNonce)
        NativeReconnectNotice.require(marker, successor.birthNonce)
        assertFalse(NativeReconnectNotice.clearIfOwner(marker, old.birthNonce))
        assertEquals(successor.birthNonce, NativeReconnectNotice.owner(marker))
        assertTrue(NativeReconnectNotice.clearIfOwner(marker, successor.birthNonce))
        assertNull(NativeReconnectNotice.owner(marker))
    }

    @Test fun explicitStopOrFreshBridgeMayClearTheExactReason() = withMarker { marker ->
        val attempt = MainKernelAttempt<Any>()
        NativeReconnectNotice.require(marker, attempt.birthNonce)
        attempt.clearReconnectNoticeOnClose = true
        assertTrue(attempt.clearReconnectNoticeOnClose)
        assertTrue(NativeReconnectNotice.clearIfOwner(marker, attempt.birthNonce))
        NativeReconnectNotice.require(marker, attempt.birthNonce)
        NativeReconnectNotice.clear(marker)
        assertNull(NativeReconnectNotice.owner(marker))
    }
}
