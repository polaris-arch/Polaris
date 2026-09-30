package com.polaris2.app.vpn

import com.polaris2.app.BuildConfig
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test

class DebugCoreProbeLoanTest {
    private class Fixture {
        val registry = MainKernelAttemptLedger()
        val witness = DebugAppliedInputWitness()
        val owner = MainKernelAttempt<Any>(runId = "pure-probe-fixture")
        val server = Any()
        val token: DebugAppliedInputWitness.Token
        val lease = DebugBatchLease()
        val ticket = checkNotNull(lease.commandBorn("probe"))
        val password = "b".repeat(32).toByteArray(Charsets.US_ASCII)
        val scope: DebugCoreProbeLoan.Scope
        init {
            check(registry.claim(owner)); owner.publish(server)
            token = witness.begin(owner, server, owner.runId, owner.birthNonce, "a".repeat(64))
            witness.returned(token, true); witness.acknowledge(owner, server, true)
            check(lease.commandEntered(ticket))
            scope = DebugCoreProbeLoan.Scope("1".repeat(32), "2".repeat(32), "3".repeat(48),
                "4".repeat(64), "5".repeat(64), "6".repeat(64), "1", owner.runId, owner.birthNonce,
                token.revision, "a".repeat(64), 9000)
        }
        fun input() = DebugCoreProbeLoan.currentInput(registry, witness)
        fun admit(binding: DebugCoreProbeLoan.Binding = DebugCoreProbeLoan.Binding(scope, 19385, 8000)) =
            DebugCoreProbeLoan.admit(scope, binding, input(), lease, ticket, 1000, password)
    }

    @Test fun actualMainReferencePairAndRevisionAreRequiredForOneCredentialUse() {
        assumeTrue(BuildConfig.DEBUG)
        val f = Fixture(); val loan = f.admit(); var writes = 0
        loan.consume(1000, f.input()) { bytes -> assertTrue(bytes === f.password); writes++ }
        assertEquals(1, writes); assertTrue(f.password.all { it == 0.toByte() })
        assertEquals(0, f.lease.snapshot().handles)
        assertTrue(runCatching { loan.consume(1000, f.input()) { writes++ } }.isFailure)
        assertEquals(1, writes)
        assertFalse(f.witness.snapshotFor(f.owner, Any()) != null)
    }

    @Test fun wrongSessionGenerationDigestRevisionAndDeadlineRejectBeforeResourceAdmission() {
        assumeTrue(BuildConfig.DEBUG)
        for (variant in 0..12) {
            val f = Fixture()
            val bad = when (variant) {
                0 -> f.scope.copy(sessionId = "7".repeat(32))
                1 -> f.scope.copy(nonce = "7".repeat(48))
                2 -> f.scope.copy(bootNonce = "7".repeat(32))
                3 -> f.scope.copy(planSha256 = "7".repeat(64))
                4 -> f.scope.copy(apkSha256 = "7".repeat(64))
                5 -> f.scope.copy(expectedSourcePin = "7".repeat(64))
                6 -> f.scope.copy(generation = "2")
                7 -> f.scope.copy(configDigest = "7".repeat(64))
                8 -> f.scope.copy(revision = f.scope.revision + 1)
                9 -> f.scope.copy(runId = "wrong-run")
                10 -> f.scope.copy(birthNonce = "wrong-birth")
                11 -> f.scope.copy(deadlineElapsed = 1000)
                else -> f.scope.copy(generation = "")
            }
            assertTrue(runCatching { f.admit(DebugCoreProbeLoan.Binding(bad, 19385, 8000)) }.isFailure)
            assertTrue(f.password.all { it == 0.toByte() }); assertEquals(0, f.lease.snapshot().handles)
            assertEquals(0, f.lease.snapshot().acquiring)
        }
        val portFixture = Fixture()
        assertTrue(runCatching { portFixture.admit(DebugCoreProbeLoan.Binding(portFixture.scope, 0, 8000)) }.isFailure)
        for (variant in 0..3) {
            val f = Fixture()
            val invalid = when (variant) {
                0 -> f.scope.copy(generation = "")
                1 -> f.scope.copy(generation = "18446744073709551616")
                2 -> f.scope.copy(deadlineElapsed = 301001)
                else -> f.scope.copy(revision = 0)
            }
            assertTrue(runCatching { DebugCoreProbeLoan.admit(invalid,
                DebugCoreProbeLoan.Binding(invalid, 19385, 8000), f.input(), f.lease, f.ticket, 1000, f.password) }.isFailure)
            assertTrue(f.password.all { it == 0.toByte() }); assertEquals(0, f.lease.snapshot().handles)
        }
    }

    @Test fun actualReloadUnknownOwnerRevokeAndServerReplacementCannotUseOrRebindOldLoan() {
        assumeTrue(BuildConfig.DEBUG)
        for (variant in 0..3) {
            val f = Fixture(); val loan = f.admit()
            when (variant) {
                0 -> f.witness.begin(f.owner, f.server, f.owner.runId, f.owner.birthNonce, "a".repeat(64))
                1 -> f.witness.failed(f.token)
                2 -> f.owner.revokeAndDetachTun()
                else -> { val t = f.witness.begin(f.owner, Any(), f.owner.runId, f.owner.birthNonce, "a".repeat(64)); f.witness.returned(t, true) }
            }
            assertFalse(loan.isCurrent(1000, f.input()))
            assertTrue(runCatching { loan.consume(1000, f.input()) { fail("changed native input reached credential consumer") } }.isFailure)
            assertTrue(f.password.all { it == 0.toByte() }); assertEquals(0, f.lease.snapshot().handles)
        }
    }

    @Test fun callbackRunsOutsideMainAndLoanLocksAndSealErasesWhileItIsActuallyBlocked() {
        assumeTrue(BuildConfig.DEBUG)
        val f = Fixture(); val loan = f.admit(); val entered = CountDownLatch(1); val release = CountDownLatch(1)
        val thread = Thread { loan.consume(1000, f.input()) {
            assertFalse(Thread.holdsLock(f.owner.operationLock)); assertFalse(Thread.holdsLock(loan))
            entered.countDown(); check(release.await(3, TimeUnit.SECONDS))
        } }
        thread.start(); assertTrue(entered.await(3, TimeUnit.SECONDS))
        f.lease.seal(); assertTrue(f.password.all { it == 0.toByte() }); assertEquals(1, f.lease.snapshot().commands)
        assertFalse(loan.isCurrent(1000, f.input()))
        release.countDown(); thread.join(3000); assertFalse(thread.isAlive)
        f.lease.commandReturned(f.ticket); assertEquals(0, f.lease.snapshot().commands)
    }

    @Test fun foreignQueuedReturnedOrSealedTicketAndExpiredLoanCannotOwnCredentials() {
        assumeTrue(BuildConfig.DEBUG)
        for (variant in 0..4) {
            val f = Fixture()
            val ticket = when (variant) {
                0 -> checkNotNull(DebugBatchLease().commandBorn("probe"))
                1 -> checkNotNull(f.lease.commandBorn("probe"))
                2 -> f.ticket.also { f.lease.commandReturned(it) }
                3 -> f.ticket.also { f.lease.seal() }
                else -> checkNotNull(f.lease.commandBorn("snapshot")).also { f.lease.commandEntered(it) }
            }
            assertTrue(runCatching { DebugCoreProbeLoan.admit(f.scope, DebugCoreProbeLoan.Binding(f.scope, 19385, 8000),
                f.input(), f.lease, ticket, 1000, f.password) }.isFailure)
            assertTrue(f.password.all { it == 0.toByte() }); assertEquals(0, f.lease.snapshot().handles)
        }
        val f = Fixture(); val loan = f.admit()
        assertTrue(runCatching { loan.consume(8000, f.input()) { fail("expired loan wrote") } }.isFailure)
        assertTrue(f.password.all { it == 0.toByte() })
    }

    @Test fun blockedActualMainOperationReadKeepsOriginalCommandAndOldTokenCannotSealReplacement() {
        assumeTrue(BuildConfig.DEBUG)
        val f = Fixture(); val entered = CountDownLatch(1); val release = CountDownLatch(1)
        val blocker = Thread { synchronized(f.owner.operationLock) { entered.countDown(); check(release.await(3, TimeUnit.SECONDS)) } }
        blocker.start(); assertTrue(entered.await(3, TimeUnit.SECONDS))
        val readStarted = CountDownLatch(1); val readReturned = CountDownLatch(1)
        val reader = Thread { readStarted.countDown(); assertNull(f.input()); readReturned.countDown() }
        reader.start(); assertTrue(readStarted.await(3, TimeUnit.SECONDS)); f.witness.seal(f.owner); f.lease.seal()
        assertEquals(1, f.lease.snapshot().commands); assertEquals(1L, readReturned.count)
        release.countDown(); blocker.join(3000); reader.join(3000); assertFalse(reader.isAlive)
        val b = Any(); val bServer = Any(); val token = f.witness.begin(b, bServer, "b", "b-birth", "b".repeat(64))
        f.witness.returned(token, true); f.witness.acknowledge(b, bServer, true)
        f.witness.returned(f.token, true); f.witness.seal(f.owner)
        assertNotNull(f.witness.snapshotFor(b, bServer))
    }
}
