package com.polaris2.app.vpn

import android.app.Activity
import app.tauri.plugin.Invoke
import com.fasterxml.jackson.annotation.JsonAutoDetect
import com.fasterxml.jackson.annotation.PropertyAccessor
import com.fasterxml.jackson.databind.DeserializationFeature
import com.fasterxml.jackson.databind.ObjectMapper
import com.polaris2.app.BuildConfig
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.json.JSONObject
import org.json.JSONArray
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test

/** Actual private manager/command/guardian call paths with pure JVM metadata/clock barriers. */
class DebugCoreProbeCommandCustodyTest {
    private class SessionFixture(val deadline: Long = 9000) : AutoCloseable {
        private val manager = DebugBatchQa
        private val active = manager.javaClass.getDeclaredField("active").also { it.isAccessible = true }
        val owner = MainKernelAttempt<Any>(runId = "pure-private-command")
        val server = Any()
        val token = DebugAppliedInputs.witness.begin(owner, server, owner.runId, owner.birthNonce, "a".repeat(64))
        private val bindingClass = Class.forName("com.polaris2.app.vpn.DebugBatchQa\$Binding")
        private val binding = bindingClass.declaredConstructors.single().also { it.isAccessible = true }
            .newInstance(owner.runId, owner.birthNonce, token.revision, "a".repeat(64))
        val sessionClass = Class.forName("com.polaris2.app.vpn.DebugBatchQa\$Session")
        val session = sessionClass.declaredConstructors.single { it.parameterCount == 8 }.also { it.isAccessible = true }
            .newInstance("4".repeat(64), "5".repeat(64), "6".repeat(64), listOf("192.168.1.1"), 47100, deadline, ByteArray(32) { 9 }, binding)
        val lease = read("lease") as DebugBatchLease
        val id = read("sessionId") as String
        init {
            check(MainKernelAttemptRegistry.claim(owner)); owner.publish(server)
            DebugAppliedInputs.witness.returned(token, true); DebugAppliedInputs.witness.acknowledge(owner, server, true)
            set("prepared", true); set("armed", true)
            val signal = Class.forName("com.polaris2.app.vpn.DebugBatchQa\$Session\$PlatformSignal")
                .declaredConstructors.single().also { it.isAccessible = true }.newInstance(1000L, true, true)
            set("signal", signal); active.set(manager, session)
        }
        fun read(name: String): Any? = sessionClass.getDeclaredField(name).also { it.isAccessible = true }.get(session)
        fun set(name: String, value: Any?) = sessionClass.getDeclaredField(name).also { it.isAccessible = true }.set(session, value)
        fun args() = DebugCoreProbeLoanArgs().also {
            it.bootNonce = read("appBootNonce") as String; it.sessionId = id; it.nonce = read("nonce") as String
            it.planSha256 = "4".repeat(64); it.apkSha256 = "5".repeat(64); it.expectedSourcePin = "6".repeat(64)
            it.generation = "1"; it.runId = owner.runId; it.birthNonce = owner.birthNonce
            it.configDigest = "a".repeat(64); it.revision = token.revision; it.deadlineElapsed = deadline
            it.probePort = 19385; it.expiresElapsed = deadline; it.password = "b".repeat(32).toByteArray()
        }
        fun expiredGuardian() {
            val unsafe = Class.forName("sun.misc.Unsafe")
            val field = unsafe.getDeclaredField("theUnsafe").also { it.isAccessible = true }
            val activity = unsafe.getMethod("allocateInstance", Class::class.java).invoke(field.get(null), Activity::class.java)
            sessionClass.getDeclaredMethod("guardTick", Activity::class.java).also { it.isAccessible = true }.invoke(session, activity)
        }
        fun closeSession(reason: String) {
            sessionClass.getDeclaredMethod("close", String::class.java).also { it.isAccessible = true }.invoke(session, reason)
        }
        override fun close() {
            closeSession("pure-fixture-close")
            if (active.get(manager) === session) active.set(manager, null)
            DebugAppliedInputs.witness.seal(owner); owner.closed.complete(null)
            MainKernelAttemptRegistry.completeAfterClose(owner) {}
        }
    }

    @Test fun actualPrivateManagerWithoutOriginalPcReadyErasesAndRejectsBeforeTransport() {
        assumeTrue(BuildConfig.DEBUG)
        SessionFixture().use { f ->
            var scope: String? = null; var report: String? = null; var rejected: Throwable? = null
            DebugBatchQa.coreProbeScopeTask(f.id, { scope = it }, { throw it }).run()
            val actual = JSONObject(checkNotNull(scope)); assertEquals(f.token.revision, actual.getLong("revision"))
            assertEquals("a".repeat(64), actual.getString("configDigest"))
            val args = f.args(); val task = DebugBatchQa.coreProbeLoanTask(args, { report = it }, { rejected = it })
            assertEquals(1, f.lease.snapshot().commands); assertEquals(1, f.lease.snapshot().handles)
            task.run()
            assertNull(report); assertEquals("Current PC Ready unavailable", rejected?.message)
            assertTrue(args.password.all { it == 0.toByte() }); assertEquals(0, f.lease.snapshot().commands)
            assertEquals(0, f.lease.snapshot().handles); assertFalse(f.read("tcpBound") as Boolean); assertFalse(f.read("udpBound") as Boolean)
        }
    }

    @Test fun actualTauriInvokeParserAcceptsRustByteArrayShapeAndOnlyManagerErasesOwnedBuffer() {
        assumeTrue(BuildConfig.DEBUG)
        SessionFixture().use { f ->
            val original = f.args()
            val body = JSONObject()
            original.javaClass.declaredFields.filter { !it.name.startsWith("$") }.forEach { field ->
                field.isAccessible = true
                val value = field.get(original)
                body.put(field.name, if (value is ByteArray) JSONArray(value.map { it.toInt() }) else value)
            }
            // The same Jackson settings as the installed PluginManager; no Activity/native call.
            val mapper = ObjectMapper().disable(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES)
                .enable(DeserializationFeature.FAIL_ON_NULL_FOR_PRIMITIVES)
                .setVisibility(PropertyAccessor.FIELD, JsonAutoDetect.Visibility.ANY)
            val invoke = Invoke(1, "debugCoreProbeLoan", 1, 2, { _, _ -> },
                JSONObject().put("loan", body).toString(), mapper)
            val parsed = invoke.parseArgs(DebugCoreProbeLoanEnvelopeArgs::class.java).loan
            assertArrayEquals(original.password, parsed.password)
            var rejected: Throwable? = null
            DebugBatchQa.coreProbeLoanTask(parsed, { fail("Missing original Ready admitted") }, { rejected = it }).run()
            assertEquals("Current PC Ready unavailable", rejected?.message)
            assertTrue(parsed.password.all { it == 0.toByte() })
            original.password.fill(0) // the fixture's independent source buffer is not an IPC loan
        }
    }

    @Test fun actualLateOriginalTaskCannotTouchNewActiveSessionOrItsInput() {
        assumeTrue(BuildConfig.DEBUG)
        val a = SessionFixture(); val args = a.args(); var failed = false
        val old = DebugBatchQa.coreProbeLoanTask(args, { fail("late A succeeded") }, { failed = true })
        a.close()
        SessionFixture().use { b ->
            val revision = DebugAppliedInputs.witness.snapshot().revision
            old.run(); assertTrue(failed); assertTrue(args.password.all { it == 0.toByte() })
            assertEquals(0, a.lease.snapshot().commands); assertFalse(b.lease.snapshot().sealed)
            assertEquals(0, b.lease.snapshot().commands)
            assertEquals(revision, DebugAppliedInputs.witness.snapshot().revision)
            assertNotNull(DebugAppliedInputs.witness.snapshotFor(b.owner, b.server))
        }
    }

    @Test fun actualBoxInputObserverFencesQueuedCredentialBeforeNativeCallbackWithoutClosingTransportUnderMainLock() {
        assumeTrue(BuildConfig.DEBUG)
        SessionFixture().use { f ->
            val args = f.args(); var failed = false; var closes = 0; var nativeCallbacks = 0
            val task = DebugBatchQa.coreProbeLoanTask(args, { fail("revoked probe ran") }, { failed = true })
            f.lease.beginAcquire(); f.lease.publish(java.io.Closeable {
                assertFalse(Thread.holdsLock(f.owner.operationLock)); closes++
            })
            val unsafe = Class.forName("sun.misc.Unsafe")
            val field = unsafe.getDeclaredField("theUnsafe").also { it.isAccessible = true }
            val box = unsafe.getMethod("allocateInstance", Class::class.java).invoke(field.get(null), BoxService::class.java)
            // No CommandServer construction or class initialization. A null opaque server makes
            // this observer Unknown; the replacement native callback is a pure JVM barrier.
            val serverType = Class.forName("io.nekohasekai.libbox.CommandServer", false, javaClass.classLoader)
            val method = BoxService::class.java.getDeclaredMethod("observeNativeInput", MainKernelAttempt::class.java,
                serverType, String::class.java, kotlin.jvm.functions.Function0::class.java, kotlin.jvm.functions.Function0::class.java)
                .also { it.isAccessible = true }
            synchronized(f.owner.operationLock) {
                method.invoke(box, f.owner, null, "{}", { true }, {
                    nativeCallbacks++; assertTrue(args.password.all { it == 0.toByte() })
                    assertTrue(f.lease.snapshot().sealed); assertEquals(2, f.lease.snapshot().handles)
                    assertEquals(1, f.lease.snapshot().commands); assertEquals(0, closes)
                })
            }
            assertEquals(1, nativeCallbacks); assertEquals("Unknown", DebugAppliedInputs.witness.snapshot().stage)
            f.expiredGuardian(); assertEquals(1, closes); assertEquals(0, f.lease.snapshot().handles)
            task.run(); assertTrue(failed); assertEquals(0, f.lease.snapshot().commands)
        }
    }

    @Test fun actualExpiredGuardianErasesQueuedCredentialAndLateCommandReturnsOnlyOriginalTicket() {
        assumeTrue(BuildConfig.DEBUG)
        SessionFixture(500).use { f ->
            val args = f.args(); var failed = false; var success = false
            val task = DebugBatchQa.coreProbeLoanTask(args, { success = true }, { failed = true })
            assertEquals(1, f.lease.snapshot().commands); assertEquals(1, f.lease.snapshot().handles)
            f.expiredGuardian()
            assertTrue(f.lease.snapshot().sealed); assertTrue(args.password.all { it == 0.toByte() })
            assertEquals(1, f.lease.snapshot().commands); assertEquals(0, f.lease.snapshot().handles)
            task.run(); assertTrue(failed); assertFalse(success); assertEquals(0, f.lease.snapshot().commands)
        }
    }

    @Test fun actualPrivateQueueRejectionRetainsTicketThroughBlockedCallbackAndNeverReportsCleanupEarly() {
        assumeTrue(BuildConfig.DEBUG)
        SessionFixture().use { f ->
            val args = f.args(); val entered = CountDownLatch(1); val release = CountDownLatch(1)
            val task = DebugBatchQa.coreProbeLoanTask(args, { fail("rejected task ran") }, {
                entered.countDown(); check(release.await(3, TimeUnit.SECONDS))
            })
            val thread = Thread { task.rejectBeforeRun(IllegalStateException("pure rejected queue")) }
            thread.start(); assertTrue(entered.await(3, TimeUnit.SECONDS))
            assertTrue(args.password.all { it == 0.toByte() }); assertEquals(1, f.lease.snapshot().commands)
            assertTrue(f.lease.snapshot().sealed); release.countDown(); thread.join(3000); assertFalse(thread.isAlive)
            task.run(); assertEquals(0, f.lease.snapshot().commands)
        }
    }

    @Test fun actualSameBirthReloadUnknownAndControllerLossRejectPrivateManagerAndErasePayload() {
        assumeTrue(BuildConfig.DEBUG)
        for (variant in 0..4) SessionFixture().use { f ->
            val args = f.args(); var failed = false
            val task = DebugBatchQa.coreProbeLoanTask(args, { fail("invalid original session admitted") }, { failed = true })
            when (variant) {
                0 -> { val t = DebugAppliedInputs.witness.begin(f.owner, f.server, f.owner.runId, f.owner.birthNonce, "a".repeat(64)); DebugAppliedInputs.witness.returned(t, true) }
                1 -> DebugAppliedInputs.witness.failed(f.token)
                2 -> f.set("controllerLeaseUntil", 999L)
                3 -> f.set("signal", null)
                else -> {
                    val locked = Class.forName("com.polaris2.app.vpn.DebugBatchQa\$Session\$PlatformSignal")
                        .declaredConstructors.single().also { it.isAccessible = true }.newInstance(1000L, false, true)
                    f.set("signal", locked)
                }
            }
            task.run(); assertTrue(failed); assertTrue(args.password.all { it == 0.toByte() })
            assertEquals(0, f.lease.snapshot().commands); assertEquals(0, f.lease.snapshot().handles)
        }
    }

    @Test fun actualSessionExpiryControllerCloseAndQueueRejectionEraseBeforeUnrelatedCloseBlocks() {
        assumeTrue(BuildConfig.DEBUG)
        val handlesField = DebugBatchLease::class.java.getDeclaredField("handles").also { it.isAccessible = true }
        val gateField = DebugBatchLease::class.java.getDeclaredField("gate").also { it.isAccessible = true }
        // Select a naturally occurring identity order, exactly as the independent review fixture.
        // No collection mutation, socket/SDK factory or synthetic close-success observation.
        for (trigger in 0..4) {
            var selected = false
            for (attempt in 0 until 128) {
                val f = SessionFixture(if (trigger == 0) 500 else 9000)
                val entered = CountDownLatch(1); val release = CountDownLatch(1)
                val error = java.util.concurrent.atomic.AtomicReference<Throwable?>()
                var calls = 0
                val blocked = java.io.Closeable {
                    assertFalse(Thread.holdsLock(f.owner.operationLock))
                    assertFalse(Thread.holdsLock(gateField.get(f.lease)))
                    calls++; entered.countDown(); check(release.await(3, TimeUnit.SECONDS))
                    if (trigger == 4) throw IllegalStateException("pure close failure")
                }
                check(f.lease.beginAcquire()); check(f.lease.publish(blocked))
                val args = f.args()
                val task = DebugBatchQa.coreProbeLoanTask(args, { fail("sealed queued task succeeded") }, {})
                var closer: Thread? = null
                try {
                    if ((handlesField.get(f.lease) as Set<*>).iterator().next() !== blocked) {
                        release.countDown(); task.rejectBeforeRun(IllegalStateException("unused natural order"))
                        continue
                    }
                    selected = true
                    if (trigger == 1) f.set("controllerLeaseUntil", 999L)
                    closer = Thread {
                        try {
                            when (trigger) {
                                0, 1 -> f.expiredGuardian()
                                2 -> f.closeSession("pure controller close")
                                else -> task.rejectBeforeRun(IllegalStateException("pure rejected queue"))
                            }
                        } catch (failure: Throwable) { error.set(failure) }
                    }
                    closer.start(); assertTrue(entered.await(3, TimeUnit.SECONDS))
                    val blockedState = f.lease.snapshot()
                    assertTrue(blockedState.sealed)
                    assertTrue("trigger=$trigger retains credential during original Close", args.password.all { it == 0.toByte() })
                    assertEquals(2, blockedState.handles); assertEquals(2, blockedState.closing)
                    assertEquals(1, blockedState.commands); assertEquals(0, blockedState.residualHandles)
                    release.countDown(); closer.join(3000); assertFalse(closer.isAlive); assertNull(error.get())
                    val returned = f.lease.snapshot()
                    assertEquals(0, returned.closing); assertEquals(if (trigger == 4) 1 else 0, returned.handles)
                    assertEquals(trigger == 4, returned.closeFailed)
                    assertEquals(if (trigger == 4) 1 else 0, returned.residualHandles)
                    assertEquals(if (trigger >= 3) 0 else 1, returned.commands)
                    task.run(); assertEquals(0, f.lease.snapshot().commands)
                    f.closeSession("repeat pure close")
                    assertEquals(1, calls) // Failed residual Close is never retried or called fake-clean.
                    println("credential seal trigger=$trigger erasedBeforeBlockedClose=true originalResponsibilityRetained=true")
                    break
                } finally {
                    release.countDown(); closer?.join(3000)
                    task.rejectBeforeRun(IllegalStateException("pure fixture cleanup")); f.close()
                }
            }
            assertTrue("natural blocked-first order was not obtained", selected)
        }
    }
}
