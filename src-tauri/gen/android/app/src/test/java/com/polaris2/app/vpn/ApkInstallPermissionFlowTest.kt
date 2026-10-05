package com.polaris2.app.vpn

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class ApkInstallPermissionFlowTest {
    @Test fun duplicatePermissionAndInstallationAreBlockedWhileSettingsAreOpen() {
        val flow = ApkInstallPermissionFlow<Any>()
        val first = Any()
        assertTrue(flow.beginPermission(first, 90))
        assertFalse(flow.beginPermission(Any(), 90))
        assertFalse(flow.beginInstall())
        assertTrue(flow.permissionPending())
        assertTrue(flow.busy())
    }

    @Test fun initialResumeDoesNotGrantAndRealReturnIsConsumedExactlyOnce() {
        val flow = ApkInstallPermissionFlow<Any>()
        val request = Any()
        assertTrue(flow.beginPermission(request, 90))
        flow.resumed()
        assertNull(flow.takeReturned())
        flow.paused()
        flow.resumed()
        assertSame(request, flow.takeReturned()?.invoke)
        assertNull(flow.takeReturned(request))
        assertFalse(flow.permissionPending())
        assertFalse(flow.busy())
    }

    @Test fun denialCanRetryAndOldCallbacksCannotSettleTheNewClick() {
        val flow = ApkInstallPermissionFlow<Any>()
        val first = Any()
        val next = Any()
        assertTrue(flow.beginPermission(first, 90))
        assertSame(first, flow.takeReturned(first)?.invoke)
        assertTrue(flow.beginPermission(next, 180))
        assertNull(flow.takeReturned(first))
        assertSame(next, flow.takeReturned(next)?.invoke)
    }

    @Test fun timeoutIsBoundToIdentityAndLateReturnCannotReopenInstallation() {
        val flow = ApkInstallPermissionFlow<Any>()
        val first = Any()
        assertTrue(flow.beginPermission(first, 90))
        assertNull(flow.expire(Any(), 100))
        assertNull(flow.expire(first, 89))
        assertSame(first, flow.expire(first, 90))
        assertNull(flow.takeReturned(first))
        assertTrue(flow.beginPermission(Any(), 180))
        assertNull(flow.expire(first, 200))
    }

    @Test fun returnedReceiptRetainsDeadlineSoLateGrantedObservationCanBeRejected() {
        val flow = ApkInstallPermissionFlow<Any>()
        val request = Any()
        assertTrue(flow.beginPermission(request, 90))
        assertEquals(90L, flow.takeReturned(request)?.deadline)
    }

    @Test fun configurationDestructionInvalidatesEveryOldCallbackAndRequiresNewClick() {
        val flow = ApkInstallPermissionFlow<Any>()
        val request = Any()
        assertTrue(flow.beginPermission(request, 90))
        assertSame(request, flow.destroy())
        assertNull(flow.takeReturned(request))
        assertNull(flow.expire(request, 100))
        assertFalse(flow.beginPermission(Any(), 180))
        assertFalse(flow.beginInstall())
        val recreated = ApkInstallPermissionFlow<Any>()
        assertNull(recreated.takeReturned(request))
        assertTrue(recreated.beginPermission(Any(), 180))
    }

    @Test fun installerHandoffBlocksDoubleLaunchUntilTheSystemActivityReturns() {
        val flow = ApkInstallPermissionFlow<Any>()
        assertTrue(flow.beginInstall())
        assertFalse(flow.beginInstall())
        flow.finishInstall(true)
        flow.resumed()
        assertFalse(flow.beginInstall())
        assertFalse(flow.beginPermission(Any(), 90))
        assertTrue(flow.busy())
        flow.paused()
        flow.resumed()
        assertTrue(flow.beginInstall())
    }

    @Test fun refusedHandoffImmediatelyPermitsRetry() {
        val flow = ApkInstallPermissionFlow<Any>()
        assertTrue(flow.beginInstall())
        flow.finishInstall(false)
        assertTrue(flow.beginInstall())
    }
}
