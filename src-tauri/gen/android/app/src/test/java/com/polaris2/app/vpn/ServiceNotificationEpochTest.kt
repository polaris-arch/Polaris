package com.polaris2.app.vpn

import java.io.File
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.*
import org.junit.Test

/** Drives the production epoch, without Android notification-manager or JNI mocks. */
class ServiceNotificationEpochTest {
    @Test fun queuedStatusAndDisconnectCannotPublishAfterStopInvalidates() {
        val epoch = ServiceNotificationEpoch()
        val token = epoch.begin()
        var traffic: String? = "current"
        var notifications = 0
        val queuedStatus = { epoch.runIfCurrent(token) { traffic = "stale"; notifications++ } }
        val queuedDisconnect = { epoch.runIfCurrent(token) { traffic = null } }
        val close = epoch.invalidate()
        assertFalse(queuedStatus())
        assertFalse(queuedDisconnect())
        assertEquals("current", traffic)
        assertEquals(0, notifications)
        assertTrue(epoch.runIfGeneration(close) { traffic = null })
        assertNull(traffic)
    }

    @Test fun restartAndScreenReconnectRejectOldClientsAndOldQueuedClose() {
        val epoch = ServiceNotificationEpoch()
        val first = epoch.begin()
        val oldClose = epoch.invalidate()
        val second = epoch.begin()
        var notifications = 0
        assertFalse(epoch.runIfCurrent(first) { notifications++ })
        assertFalse(epoch.runIfGeneration(oldClose) { fail("old close must not remove new foreground") })
        assertTrue(epoch.runIfCurrent(second) { notifications++ })
        epoch.invalidate() // Screen off disconnects exactly this client's epoch.
        val reconnected = epoch.begin()
        assertFalse(epoch.runIfCurrent(second) { notifications++ })
        assertTrue(epoch.runIfCurrent(reconnected) { notifications++ })
        assertEquals(2, notifications)
    }

    @Test fun invalidationAndActualPublicationAreSerializedNotJustEntryChecked() {
        val epoch = ServiceNotificationEpoch()
        val token = epoch.begin()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val stopping = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>()
        val order = Collections.synchronizedList(mutableListOf<String>())
        fun worker(action: () -> Unit) = Thread {
            try { action() } catch (error: Throwable) { failure.compareAndSet(null, error) }
        }.also { it.start() }
        val publishing = worker {
            assertTrue(epoch.runIfCurrent(token) {
                entered.countDown()
                assertTrue(release.await(5, TimeUnit.SECONDS))
                order += "publish"
            })
        }
        assertTrue(entered.await(5, TimeUnit.SECONDS))
        val stoppingWorker = worker {
            stopping.countDown()
            epoch.invalidate()
            order += "stop"
        }
        try {
            assertTrue(stopping.await(5, TimeUnit.SECONDS))
        } finally { release.countDown() }
        publishing.join(5000); stoppingWorker.join(5000)
        assertFalse(publishing.isAlive); assertFalse(stoppingWorker.isAlive)
        failure.get()?.let { throw AssertionError(it) }
        assertEquals(listOf("publish", "stop"), order.toList())
        assertFalse(epoch.runIfCurrent(token) { fail("no publication after stop returned") })
    }

    @Test fun initialForegroundPublicationRechecksAttemptUnderTheStopLock() {
        val service = File("src/main/java/com/polaris2/app/vpn/BoxService.kt").readText()
        val start = service.substringAfter("// 先立前台通知").substringBefore("Thread({ startKernel(attempt) }")
        val lock = start.indexOf("synchronized(this)")
        val current = start.indexOf("if (!isStarting(attempt) || !MainKernelAttemptRegistry.isCurrent(attempt))")
        val show = start.indexOf("notification.show(ServiceState.Starting, PROFILE_NAME)")
        assertTrue(lock >= 0 && lock < current && current < show)
        val rejected = start.substring(current, show)
        assertTrue(rejected.contains("attempt.skipPreparation()"))
        assertTrue(rejected.contains("service.stopSelfResult(startId)"))
        assertTrue(rejected.contains("return Service.START_NOT_STICKY"))
    }

    @Test fun productionChecksEpochAtHandlerExecutionAndInvalidatesBeforeQueuedClose() {
        val notification = File("src/main/java/com/polaris2/app/vpn/ServiceNotification.kt").readText()
        val service = File("src/main/java/com/polaris2/app/vpn/BoxService.kt").readText()
        val status = notification.substringAfter("override fun writeStatus(").substringBefore("override fun clearLogs(")
        val post = status.indexOf("handler.post")
        val guard = status.indexOf("epoch.runIfCurrent(token)")
        val notify = status.indexOf("PolarisApplication.notification.notify")
        assertTrue(post >= 0 && post < guard && guard < notify)
        val disconnected = notification.substringAfter("override fun disconnected(").substringBefore("override fun writeStatus(")
        assertTrue(disconnected.contains("epoch.runIfCurrent(token)"))
        assertTrue(notification.contains("StatusHandler(token)"))
        assertTrue(notification.contains("state = ServiceState.Stopped"))
        assertTrue(notification.contains("epoch.runIfGeneration(expectedEpoch)"))
        assertTrue(notification.contains("override fun onReceive(context: Context, intent: Intent) = synchronized(epoch)"))
        val stop = service.substringAfter("if (firstStop) {").substringBefore("attempt.closed.whenComplete")
        val invalidate = stop.indexOf("notification.invalidateForStop()")
        val queue = stop.indexOf("mainHandler.post")
        val close = stop.indexOf("notification.close(notificationEpoch)")
        assertTrue(invalidate >= 0 && invalidate < queue && queue < close)
    }
}
