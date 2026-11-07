package com.polaris2.app.vpn

import java.io.Closeable
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.Assert.*
import org.junit.Test

class NativeTunOwnerTest {
    private class Receipt(override val ownerId: String) : NativeTunReceipt {
        var committed = false
        override fun published() = committed
    }
    private class Descriptor(val number: Int) : Closeable {
        var closes = 0
        var failClose = false
        override fun close() { closes++; check(!failClose) { "close failed" } }
    }
    private class Harness {
        var revoked = false
        var owns = true
        var unknown = 0
        var failEstablish = false
        var nextEstablish: (() -> Unit)? = null
        val settings = mutableListOf<String>()
        val descriptors = mutableListOf<Descriptor>()
        val owner = NativeTunOwner<String, Descriptor>(
            current = { check(owns && !revoked) }, stillOwner = { check(owns) },
            establish = { name ->
                settings.add(name)
                nextEstablish?.also { nextEstablish = null }?.invoke()
                check(!failEstablish) { "establish failed" }
                Descriptor(descriptors.size + 10).also(descriptors::add)
            }, fd = { it.number }, scope = { scope(it) }, onUnknown = { unknown++ },
        )
        fun committed(name: String = "old"): Pair<NativeTunSession, Receipt> {
            val receipt = Receipt(name)
            val session = owner.newSession(name, null, receipt)
            session.prepare(); session.validate(); receipt.committed = true
            return session to receipt
        }
    }
    companion object {
        private fun scope(name: String) = TunScope(true, listOf(name), emptyList(), emptyList(), emptyList(), emptyList(), emptyList())
    }

    @Test fun preparePublicationRetirementUseActualOriginalsAndExactGraph() {
        val h = Harness()
        val (old, oldReceipt) = h.committed()
        val receipt = Receipt("new")
        oldReceipt.committed = false // Real Core suspends/join-closes the previous physical first.
        val next = h.owner.newSession("new", old, receipt)
        assertEquals(listOf("old"), h.settings) // allocation has no establish effect
        assertTrue(runCatching { h.owner.newSession("foreign", null, Receipt("foreign")) }.isFailure)
        next.prepare()
        assertEquals(11, next.fileDescriptor())
        assertEquals(null, h.owner.currentScope()) // Neither physical is currently published.
        next.validate(); receipt.committed = true
        assertEquals(scope("new"), h.owner.currentScope())
        old.retire(); old.retire()
        assertEquals(1, h.descriptors[0].closes)
        assertEquals(0, h.descriptors[1].closes)
        h.revoked = true // Stop fences admission, exact native Retire remains callable after join.
        assertTrue(runCatching { next.validate() }.isFailure)
        receipt.committed = false
        next.retire(); h.owner.close()
        assertEquals(1, h.descriptors[1].closes)
    }

    @Test fun abortReestablishesOldSettingsAndReturnsItsNewDescriptor() {
        val h = Harness()
        val (old, oldReceipt) = h.committed()
        oldReceipt.committed = false
        val candidate = h.owner.newSession("new", old, Receipt("new"))
        candidate.prepare(); candidate.abort(); candidate.abort()
        assertEquals(listOf("old", "new", "old"), h.settings)
        assertFalse(oldReceipt.published())
        assertEquals(null, h.owner.currentScope())
        old.validate() // Real libbox rebuild validates the restored exact owner before re-publication.
        assertEquals(12, old.fileDescriptor()) // Core rebuild must consume this fd, never stale old fd.
        assertEquals(1, h.descriptors[0].closes)
        assertEquals(1, h.descriptors[1].closes)
        assertEquals(0, h.descriptors[2].closes)
        oldReceipt.committed = true // Original Core publication participant reopens the rebuilt physical.
        assertEquals(scope("old"), h.owner.currentScope())
    }

    @Test fun failedEstablishStillRequiresOldSettingsRestoration() {
        val h = Harness()
        val (old, oldReceipt) = h.committed()
        oldReceipt.committed = false
        val candidate = h.owner.newSession("new", old, Receipt("new"))
        h.failEstablish = true
        assertTrue(runCatching { candidate.prepare() }.isFailure)
        h.failEstablish = false
        candidate.abort()
        assertEquals(listOf("old", "new", "old"), h.settings)
        assertEquals(11, old.fileDescriptor())
        assertEquals(0, h.unknown)
    }

    @Test fun restorationFailureQuarantinesOriginalOwnerAndAllNewWork() {
        val h = Harness()
        val (old, oldReceipt) = h.committed()
        oldReceipt.committed = false
        val candidate = h.owner.newSession("new", old, Receipt("new"))
        candidate.prepare(); h.failEstablish = true
        assertTrue(runCatching { candidate.abort() }.isFailure)
        assertEquals(1, h.unknown)
        assertTrue(runCatching { candidate.abort() }.isFailure)
        assertTrue(runCatching { old.fileDescriptor() }.isFailure)
        assertTrue(runCatching { h.owner.newSession("later", old, Receipt("later")) }.isFailure)
        assertTrue(runCatching { h.owner.close() }.isFailure)
        assertEquals(listOf(0, 0), h.descriptors.map { it.closes })
    }

    @Test fun stopDuringEstablishRejectsLateResultAndCannotClaimOldSettingsRestored() {
        val h = Harness()
        val (old, oldReceipt) = h.committed()
        oldReceipt.committed = false
        val candidate = h.owner.newSession("new", old, Receipt("new"))
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val finished = CountDownLatch(1)
        var failed = false
        h.nextEstablish = { entered.countDown(); check(release.await(2, TimeUnit.SECONDS)) }
        Thread { failed = runCatching { candidate.prepare() }.isFailure; finished.countDown() }.start()
        assertTrue(entered.await(2, TimeUnit.SECONDS))
        h.revoked = true
        assertEquals(0, h.descriptors[0].closes)
        release.countDown(); assertTrue(finished.await(2, TimeUnit.SECONDS)); assertTrue(failed)
        assertTrue(runCatching { candidate.validate() }.isFailure)
        // Core has joined candidate readers before Abort. Revoked means old settings cannot be restored.
        assertTrue(runCatching { candidate.abort() }.isFailure)
        assertEquals(1, h.unknown)
        assertTrue(runCatching { h.owner.close() }.isFailure)
        assertEquals(listOf(0, 0), h.descriptors.map { it.closes })
    }

    @Test fun retireFailureCannotBeRetriedIntoFalseCleanupSuccess() {
        val h = Harness()
        val (old, oldReceipt) = h.committed()
        oldReceipt.committed = false
        h.descriptors[0].failClose = true
        assertTrue(runCatching { old.retire() }.isFailure)
        h.descriptors[0].failClose = false
        assertTrue(runCatching { old.retire() }.isFailure)
        assertTrue(runCatching { h.owner.close() }.isFailure)
        assertEquals(1, h.descriptors[0].closes)
        assertEquals(1, h.unknown)
    }

    @Test fun exactAttemptKeepsGraphCustodyThroughRevokeAndRejectsLegacyReplacement() {
        val attempt = MainKernelAttempt<Any>()
        val registry = MainKernelAttemptLedger()
        assertTrue(registry.claim(attempt))
        var closes = 0
        var scopeReads = 0
        val custody = Closeable { closes++ }
        registry.bindTunOwner(attempt, custody) { scopeReads++; scope("actual") }
        assertEquals(scope("actual"), attempt.currentTunScope())
        assertTrue(runCatching { registry.installTun(attempt, Closeable { }, scope("foreign")) }.isFailure)
        val detached = attempt.revokeAndDetachTun()
        assertSame(custody, detached)
        assertEquals(null, attempt.currentTunScope())
        assertEquals(1, scopeReads)
        assertEquals(0, closes) // Native join precedes the Stop job's terminal original-FD close.
        attempt.publish(Any())
        attempt.closeOnce { detached?.close() }
        assertEquals(null, attempt.closed.get(2, TimeUnit.SECONDS))
        assertEquals(1, closes)
        assertTrue(registry.completeAfterClose(attempt) { })
    }

    @Test fun unpreparedAbortHasNoSettingsEffectAndInitialAbortClosesItsOwnOriginal() {
        val h = Harness()
        val abandoned = h.owner.newSession("never", null, Receipt("never"))
        abandoned.abort(); assertTrue(h.settings.isEmpty())
        val candidate = h.owner.newSession("initial", null, Receipt("initial"))
        candidate.prepare(); candidate.abort()
        assertEquals(listOf("initial"), h.settings)
        assertEquals(1, h.descriptors.single().closes)
        assertEquals(null, h.owner.currentScope())
    }
}
