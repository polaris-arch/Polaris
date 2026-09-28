package com.polaris2.app.vpn

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LegacyAdmissionFenceTest {
    private fun waitFor(latch: CountDownLatch) = assertTrue(latch.await(2, TimeUnit.SECONDS))

    @Test fun onlyFenceRejectedUnownedServiceStopsItself() {
        val registry = MainKernelAttemptLedger()
        val gate = LegacyAdmissionFence<String>(
            detachPending = { null },
            bridgeIdle = { true },
            currentOwner = registry::ownerForDrain,
        )
        // An open gate can still yield no attempt (for example, another owner
        // occupies the registry); that is not a fence rejection.
        val ordinaryMiss = gate.admitWithDecision<Any?> { null }
        assertFalse(ordinaryMiss.rejectedByFence)
        assertFalse(shouldStopSelfAfterFenceRejection(
            ordinaryMiss.rejectedByFence, ServiceState.Stopped, hasAttempt = false,
        ))

        gate.begin("fgs-fence")
        val fenced = gate.admitWithDecision { error("fenced action must not run") }
        assertTrue(fenced.rejectedByFence)
        assertTrue(shouldStopSelfAfterFenceRejection(
            fenced.rejectedByFence, ServiceState.Stopped, hasAttempt = false,
        ))
        assertFalse(shouldStopSelfAfterFenceRejection(
            fenced.rejectedByFence, ServiceState.Started, hasAttempt = true,
        ))
        assertFalse(shouldStopSelfAfterFenceRejection(
            fenced.rejectedByFence, ServiceState.Starting, hasAttempt = true,
        ))
        assertFalse(shouldStopSelfAfterFenceRejection(
            fenced.rejectedByFence, ServiceState.Stopped, hasAttempt = true,
        ))
    }

    @Test fun pendingBridgeRequestBeforeServiceIsDetachedBeforeVacant() {
        val registry = MainKernelAttemptLedger()
        var pending: String? = "bridge-run"
        var bridgeBusy = true
        val gate = LegacyAdmissionFence(
            detachPending = { pending.also { pending = null; bridgeBusy = false } },
            bridgeIdle = { !bridgeBusy && pending == null },
            currentOwner = registry::ownerForDrain,
        )
        val beginning = gate.begin("fence-1")
        assertEquals("bridge-run", beginning.pendingToReject)
        assertNull(beginning.ownerToClose)
        assertEquals("vacant", gate.status("fence-1").state)
        assertNull(gate.admit { registry.claim(MainKernelAttempt<Any>()) })
        assertEquals("absent" to null, registry.snapshot())
    }

    @Test fun claimAndFenceSerializeBeforeNativeEntry() {
        val registry = MainKernelAttemptLedger()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val attempt = MainKernelAttempt<Any>(runId = "system-exact")
        val gate = LegacyAdmissionFence<String>(
            detachPending = { null },
            bridgeIdle = { false }, // a system attempt is in progress
            currentOwner = registry::ownerForDrain,
        )
        val claiming = Thread {
            gate.admit {
                assertTrue(registry.claim(attempt) { })
                entered.countDown()
                waitFor(release)
            }
        }
        claiming.start()
        waitFor(entered)
        val begun = AtomicReference<LegacyAdmissionFence.Beginning<String>>()
        val fencing = Thread { begun.set(gate.begin("fence-2")) }
        fencing.start()
        assertFalse(fencing.joinWithin(30))
        release.countDown()
        claiming.join()
        fencing.join()
        assertEquals("system-exact", begun.get().ownerToClose?.attempt?.runId)
        assertEquals("owned", gate.status("fence-2").state)
        assertNull(gate.admit { true })
    }

    @Test fun exactCloseMustFinishAndReleaseBeforeVacant() {
        val registry = MainKernelAttemptLedger()
        val attempt = MainKernelAttempt<Any>(runId = "old-run")
        val closeEntered = CountDownLatch(1)
        val releaseClose = CountDownLatch(1)
        assertTrue(registry.claim(attempt) {
            attempt.revokeAndDetachTun()
            attempt.closeOnce {
                closeEntered.countDown()
                waitFor(releaseClose)
            }
        })
        attempt.publish(Any())
        var busy = true
        val gate = LegacyAdmissionFence<String>(
            detachPending = { null },
            bridgeIdle = { !busy },
            currentOwner = registry::ownerForDrain,
        )
        val first = gate.begin("fence-3")
        first.ownerToClose?.requestClose?.invoke()
        waitFor(closeEntered)
        assertEquals("owned", gate.status("fence-3").state)
        assertEquals("unknown", gate.await("fence-3", 30, TimeUnit.MILLISECONDS).state)
        releaseClose.countDown()
        assertNull(attempt.closed.get(2, TimeUnit.SECONDS))
        assertEquals("owned", gate.status("fence-3").state) // native close alone is insufficient
        busy = false
        assertTrue(registry.completeAfterClose(attempt) { })
        assertEquals("old-run", gate.status("fence-3").closedRunId)
        assertEquals("vacant", gate.status("fence-3").state)
        assertEquals("old-run", gate.begin("fence-3").ownerToClose?.attempt?.runId)
        assertEquals("unknown", gate.status("different-fence").state)
    }

    @Test fun cleanupFailureRetainsOwnerAndNeverBecomesVacant() {
        val registry = MainKernelAttemptLedger()
        val attempt = MainKernelAttempt<Any>(runId = "failed-close")
        assertTrue(registry.claim(attempt) {
            attempt.revokeAndDetachTun()
            attempt.closeOnce { error("native close failed") }
        })
        attempt.publish(Any())
        val gate = LegacyAdmissionFence<String>(
            detachPending = { null },
            bridgeIdle = { false },
            currentOwner = registry::ownerForDrain,
        )
        gate.begin("fence-4").ownerToClose?.requestClose?.invoke()
        assertEquals("native close failed", attempt.closed.get(2, TimeUnit.SECONDS)?.message)
        assertEquals("unknown", gate.status("fence-4").state)
        assertEquals("cleanup-unknown", gate.status("fence-4").reason)
        assertFalse(registry.claim(MainKernelAttempt<Any>()))
    }

    @Test fun finalAdmissionCheckRacingNativeEntryCannotReleaseOwnerEarly() {
        val registry = MainKernelAttemptLedger()
        val attempt = MainKernelAttempt<Any>(runId = "native-in-flight")
        val nativeEntered = CountDownLatch(1)
        val leaveNative = CountDownLatch(1)
        var bridgeBusy = true
        val gate = LegacyAdmissionFence<String>(
            detachPending = { null },
            bridgeIdle = { !bridgeBusy },
            currentOwner = registry::ownerForDrain,
        )
        assertTrue(registry.claim(attempt) {
            attempt.revokeAndDetachTun()
            attempt.closeOnce {
                // BoxService terminal close uses the same lock as Start/Reload.
                synchronized(attempt.operationLock) { }
            }
        })
        attempt.publish(Any())
        val native = Thread {
            synchronized(attempt.operationLock) {
                gate.requireOpen() // the last short A check was passed before the fence
                nativeEntered.countDown()
                waitFor(leaveNative)
            }
        }
        native.start()
        waitFor(nativeEntered)
        gate.begin("fence-native").ownerToClose?.requestClose?.invoke()
        assertEquals("unknown", gate.await("fence-native", 30, TimeUnit.MILLISECONDS).state)
        assertFalse(attempt.closed.isDone)
        leaveNative.countDown()
        native.join()
        assertNull(attempt.closed.get(2, TimeUnit.SECONDS))
        bridgeBusy = false
        assertTrue(registry.completeAfterClose(attempt) { })
        assertEquals("vacant", gate.status("fence-native").state)
        assertEquals("native-in-flight", gate.status("fence-native").closedRunId)
    }

    @Test fun beginSystemStartReentersAdmissionMonitorWithoutReversingLocks() {
        val admitted = LegacySystemStartFence.admit { VpnBridge.beginSystemStart() }
        assertEquals(true, admitted)
        VpnBridge.finishStop()
    }

    private fun Thread.joinWithin(timeoutMs: Long): Boolean {
        join(timeoutMs)
        return !isAlive
    }
}
