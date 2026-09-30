package com.polaris2.app.vpn

/** Main-core adapter: one ticket spans bridge pending, Service attempt, and exact close. */
internal object AndroidNativeMain {
    val capabilities = setOf(
        AndroidNativeProducer.MainBridge,
        AndroidNativeProducer.MainSystem,
    )

    val controls: AndroidNativeMainControls
        get() = AndroidNativeMainControls(AndroidNativeAdmissionGate.ledger)

    fun reserveBridge(runId: String): AndroidNativeAdmission.Ticket =
        AndroidNativeAdmissionGate.ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, runId)

    fun reserveSystem(runId: String): AndroidNativeAdmission.Ticket = reserveBridge(runId)

    fun enterBirth(ticket: AndroidNativeAdmission.Ticket) = enterBirth(AndroidNativeAdmissionGate.ledger, ticket)

    internal fun enterBirth(ledger: AndroidNativeAdmission, ticket: AndroidNativeAdmission.Ticket) {
        require(ticket.kind == AndroidNativeAdmission.Kind.Main)
        if (!ledger.enterBirth(ticket)) {
            throw ledger.admissionRejection()
        }
    }

    fun cancelBeforeBirth(ticket: AndroidNativeAdmission.Ticket) {
        require(ticket.kind == AndroidNativeAdmission.Kind.Main)
        AndroidNativeAdmissionGate.ledger.cancelBeforeBirth(ticket)
    }

    fun unknown(attempt: MainKernelAttempt<*>) {
        attempt.nativeTicket?.let(AndroidNativeAdmissionGate.ledger::unknown)
    }

    /** Record failure/cancellation before Stop can acquire the same operation lock. */
    fun <T> construct(attempt: MainKernelAttempt<*>, action: () -> T): T =
        construct(AndroidNativeAdmissionGate.ledger, attempt, action)

    internal fun <T> construct(ledger: AndroidNativeAdmission, attempt: MainKernelAttempt<*>, action: () -> T): T =
        synchronized(attempt.operationLock) {
            try {
                val result = action()
                check(!attempt.revoked) { "android: native construction completion was revoked" }
                result
            } catch (failure: Throwable) {
                attempt.markConstructionUnknown()
                attempt.nativeTicket?.let(ledger::unknown)
                throw failure
            }
        }

    /** Operational close releases the registry; global DNS and native leases remain unproved. */
    fun settleAfterExactRelease(attempt: MainKernelAttempt<*>) =
        settleAfterExactRelease(AndroidNativeAdmissionGate.ledger, attempt)

    internal fun settleAfterExactRelease(ledger: AndroidNativeAdmission, attempt: MainKernelAttempt<*>) {
        val ticket = attempt.nativeTicket ?: return
        if (attempt.constructionUnknown || !attempt.closeSucceeded() ||
            !attempt.released.isDone || attempt.released.isCompletedExceptionally ||
            !attempt.prepared.isDone || attempt.prepared.isCompletedExceptionally) {
            ledger.unknown(ticket)
            return
        }
        if (attempt.prepared.getNow(null) != null) {
            // Ordinary CommandServer Close does not prove process-global resolver
            // drainage or all native leases. Never promote a born main owner.
            ledger.unknown(ticket)
        } else if (!ledger.cancelBeforeBirth(ticket)) {
            ledger.unknown(ticket)
        }
    }
}
