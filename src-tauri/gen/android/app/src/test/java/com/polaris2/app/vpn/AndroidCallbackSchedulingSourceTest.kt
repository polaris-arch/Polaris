package com.polaris2.app.vpn

import java.io.File
import org.junit.Assert.*
import org.junit.Test

/** Wiring guards only. The held native/SDK sequence fixtures are separate acceptance work. */
class AndroidCallbackSchedulingSourceTest {
    private fun source(name: String) = File("src/main/java/com/polaris2/app/vpn/$name.kt").readText()

    @Test fun actualNativeStatsHandlerIsSubscriptionBound() {
        val stats = source("StatsBridge")
        assertTrue(stats.contains("Libbox.newCommandClient(SubscriptionHandler(token), options)"))
        assertFalse(stats.contains("Libbox.newCommandClient(this@CommandStream, options)"))
        val wrapper = stats.substringAfter("private inner class SubscriptionHandler")
            .substringBefore("// ── CommandClientHandler")
        assertTrue(wrapper.contains("subscriptionToken === token && serviceReady && closedReason == null"))
        for (callback in listOf("connected()", "disconnected(message: String?)",
            "writeStatus(message: StatusMessage)", "writeConnectionEvents(events: ConnectionEvents?)")) {
            assertTrue(wrapper.contains("override fun $callback = current"))
        }
        assertTrue(stats.contains("subscriptionToken.also { markClosed() }"))
        assertTrue(stats.contains("worker.execute { closeClient(token) }"))
        assertTrue(stats.contains("if (ownsSubscription) closeClient(token)"))
    }

    @Test fun guardObservationHasOriginalSessionCustodyAndIndependentBoundedExecutor() {
        val qa = source("DebugBatchQa")
        val guard = qa.substringAfter("fun guardTick(activity: Activity)").substringBefore("private fun samplePlatform")
        assertTrue(guard.contains("signalPending.compareAndSet(false, true)"))
        assertTrue(guard.contains("lease.workerBorn()"))
        assertTrue(guard.contains("observer.execute(task)"))
        assertFalse(guard.contains("DebugBatchCommandExecutor"))
        assertTrue(qa.contains("ArrayBlockingQueue<Runnable>(1)"))
        assertTrue(qa.contains("observer.shutdownNow().forEach { (it as DebugBatchTask).rejectBeforeRun() }"))
        assertTrue(qa.contains("observer.isTerminated"))
        assertTrue(guard.contains("DebugBatchGuard.abortReason"))
    }

    @Test fun networkAndNotificationCallbacksOnlyAdmitOffMainNativeWork() {
        val monitor = source("NetworkMonitorSession")
        assertTrue(monitor.contains("session.worker.execute { pump(session) }"))
        assertTrue(monitor.contains("if (session.pumpRunning) return false"))
        assertTrue(monitor.contains("session.current !== listener || session.revision != work.revision"))
        val notification = source("ServiceNotification")
        val request = notification.substringAfter("private fun connectStatusClient()").substringBefore("private fun pumpStatusClient()")
        assertFalse(request.contains(".connect()"))
        assertFalse(request.contains(".disconnect()"))
        assertTrue(request.contains("statusWorker.execute { pumpStatusClient() }"))
        val stop = notification.substringAfter("fun invalidateForStop()").substringBefore("fun close(")
        assertTrue(stop.contains("desiredStatusToken = null"))
        assertTrue(stop.contains("scheduleStatusPump()"))
        assertTrue(notification.contains("if (!installed) runCatching { client?.disconnect() }"))
    }
}
