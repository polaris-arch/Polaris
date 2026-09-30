package com.polaris2.app.vpn

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Device test: the login worker must observe a main claim while native Start is blocked. */
class TransientLoginMainClaimTest {
    private fun await(latch: CountDownLatch) = assertTrue(latch.await(3, TimeUnit.SECONDS))
    private val config = """{"endpoints":[{"type":"tailscale","tag":"login-test","state_directory":"/data/local/tmp/polaris-main-claim-test"}]}"""

    @Test fun stopCanCloseBlockedMainActionAndLateReturnCannotRestoreItsClaim() {
        val attempt = MainKernelAttempt<Any>()
        val actionEntered = CountDownLatch(1)
        val releaseAction = CountDownLatch(1)
        val actionResult = AtomicReference<Throwable?>()
        val actionDone = CountDownLatch(1)
        Thread {
            actionResult.set(runCatching {
                TransientLoginHost.withMainConfig(attempt, config, { !attempt.revoked }) {
                    actionEntered.countDown()
                    await(releaseAction)
                }
            }.exceptionOrNull())
            actionDone.countDown()
        }.start()
        await(actionEntered)

        val loginDone = CountDownLatch(1)
        val loginFailure = AtomicReference<TransientLoginHost.StartFailure?>()
        TransientLoginHost.start("claim-race-${System.nanoTime()}", config) {
            loginFailure.set(it)
            loginDone.countDown()
        }
        await(loginDone)
        assertTrue(loginFailure.get() is TransientLoginHost.GeneralFailure)
        assertTrue(loginFailure.get()!!.message.contains("ownership"))

        attempt.revokeAndDetachTun()
        val closeEntered = CountDownLatch(1)
        val closeDone = CountDownLatch(1)
        Thread {
            TransientLoginHost.closeMain(attempt) { closeEntered.countDown() }
            closeDone.countDown()
        }.start()
        await(closeEntered)
        await(closeDone)
        assertFalse(actionDone.await(50, TimeUnit.MILLISECONDS))
        releaseAction.countDown()
        await(actionDone)
        assertTrue(actionResult.get() is IllegalStateException)
    }
}
