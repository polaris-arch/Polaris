package com.polaris2.app.vpn

import java.util.concurrent.atomic.AtomicBoolean

/** One callback's work, separate from the main owner that survives a successful reload. */
internal class AndroidNativeControlOperation private constructor(
    private val ledger: AndroidNativeAdmission,
    val ticket: AndroidNativeAdmission.Ticket,
) {
    private val dispatched = AtomicBoolean(false)

    class Revoked : IllegalStateException("android: native control target was revoked")

    /** Failed queue dispatch has acquired no native resources. */
    fun cancelBeforeDispatch() {
        if (dispatched.compareAndSet(false, true)) ledger.cancelBeforeBirth(ticket)
    }

    /** The caller supplies an exact attempt/server check, never a lookup of the newest owner. */
    fun runExact(
        attempt: MainKernelAttempt<*>,
        allowed: () -> Boolean,
        action: (Boundary) -> Unit,
    ): Boolean = synchronized(attempt.operationLock) {
        check(dispatched.compareAndSet(false, true)) { "native control was already dispatched" }
        val boundary = Boundary(attempt, allowed)
        try {
            if (attempt.revoked || !allowed()) {
                ledger.cancelBeforeBirth(ticket)
                return@synchronized false
            }
            action(boundary)
            if (boundary.entered && (attempt.revoked || !allowed())) throw Revoked()
            if (boundary.entered) {
                if (!ledger.completeOperation(ticket)) ledger.unknown(ticket)
            } else ledger.cancelBeforeBirth(ticket)
            true
        } catch (failure: Throwable) {
            if (boundary.entered) ledger.unknown(ticket)
            else ledger.cancelBeforeBirth(ticket)
            boundary.rememberFailure(failure)
            throw checkNotNull(boundary.firstFailure)
        } finally {
            boundary.active = false
        }
    }

    inner class Boundary internal constructor(
        private val attempt: MainKernelAttempt<*>,
        private val allowed: () -> Boolean,
    ) {
        internal var entered = false
        internal var active = true
        internal var firstFailure: Throwable? = null
            private set

        internal fun rememberFailure(failure: Throwable) {
            val first = firstFailure
            if (first == null) firstFailure = failure
            else if (first !== failure && first.suppressed.none { it === failure }) first.addSuppressed(failure)
        }

        private fun requireCurrent() {
            check(active && Thread.holdsLock(attempt.operationLock)) { "native control boundary has ended" }
            if (attempt.revoked || !allowed()) throw Revoked()
        }

        private fun enterNative() {
            requireCurrent()
            if (!entered) {
                if (!ledger.enterBirth(ticket)) throw ledger.admissionRejection()
                entered = true
            }
        }

        /** Also guards error-report JNI; reporting a failed reload cannot erase its Unknown fact. */
        fun <T> native(action: () -> T): T {
            enterNative()
            try {
                return action()
            } catch (failure: Throwable) {
                ledger.unknown(ticket)
                rememberFailure(failure)
                throw failure
            }
        }

        /** Only construction uncertainty poisons the main owner's later exact-close proof. */
        fun <T> construct(action: () -> T): T {
            require(ticket.kind == AndroidNativeAdmission.Kind.TargetlessReload)
            enterNative()
            try {
                return AndroidNativeMain.construct(ledger, attempt) {
                    val result = action()
                    requireCurrent()
                    result
                }
            } catch (failure: Throwable) {
                ledger.unknown(ticket)
                rememberFailure(failure)
                throw failure
            }
        }

        /** Keep the first error even if setError itself is rejected, revoked, or throws. */
        fun reportFailure(firstFailure: Throwable, report: () -> Unit) {
            rememberFailure(firstFailure)
            try { native(report) }
            catch (reportFailure: Throwable) { rememberFailure(reportFailure) }
        }

        /** Pure preflight/reconnect notification; its adapter still checks the exact notice owner. */
        fun ifCurrent(action: () -> Unit): Boolean {
            check(active && Thread.holdsLock(attempt.operationLock)) { "native control boundary has ended" }
            if (attempt.revoked || !allowed()) return false
            action()
            return true
        }
    }

    companion object {
        fun reserve(ledger: AndroidNativeAdmission, kind: AndroidNativeAdmission.Kind): AndroidNativeControlOperation {
            require(kind == AndroidNativeAdmission.Kind.TargetlessStop ||
                kind == AndroidNativeAdmission.Kind.TargetlessReload)
            return AndroidNativeControlOperation(ledger, ledger.reserveOperation(kind))
        }

        /** Missing provenance cannot be repaired by choosing whatever owner exists at delivery. */
        fun rejectUnbound(ledger: AndroidNativeAdmission, kind: AndroidNativeAdmission.Kind): AndroidNativeAdmission.Ticket {
            val operation = reserve(ledger, kind)
            operation.cancelBeforeDispatch()
            return operation.ticket
        }
    }
}
