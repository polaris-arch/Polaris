package com.polaris2.app.vpn

/** Main-core adapter: one ticket spans bridge pending, Service attempt, and exact close. */
internal object AndroidNativeMain {
    val capabilities = setOf(
        AndroidNativeProducer.MainBridge,
        AndroidNativeProducer.MainSystem,
        AndroidNativeProducer.MainClose,
    )

    fun reserveBridge(runId: String): AndroidNativeAdmission.Ticket =
        AndroidNativeAdmissionGate.ledger.reserveOwner(AndroidNativeAdmission.Kind.Main, runId)

    fun reserveSystem(runId: String): AndroidNativeAdmission.Ticket = reserveBridge(runId)

    fun enterBirth(ticket: AndroidNativeAdmission.Ticket) {
        require(ticket.kind == AndroidNativeAdmission.Kind.Main)
        if (!AndroidNativeAdmissionGate.ledger.enterBirth(ticket)) {
            throw AndroidNativeAdmission.AdmissionClosed()
        }
    }

    fun cancelBeforeBirth(ticket: AndroidNativeAdmission.Ticket) {
        require(ticket.kind == AndroidNativeAdmission.Kind.Main)
        AndroidNativeAdmissionGate.ledger.cancelBeforeBirth(ticket)
    }

    fun unknown(attempt: MainKernelAttempt<*>) {
        attempt.nativeTicket?.let(AndroidNativeAdmissionGate.ledger::unknown)
    }

    /** Caller first confirms native CloseService/Close, then releases this exact registry owner. */
    fun settleAfterExactRelease(attempt: MainKernelAttempt<*>) =
        settleAfterExactRelease(AndroidNativeAdmissionGate.ledger, attempt)

    internal fun settleAfterExactRelease(ledger: AndroidNativeAdmission, attempt: MainKernelAttempt<*>) {
        val ticket = attempt.nativeTicket ?: return
        if (!attempt.closed.isDone || attempt.closed.getNow(null) != null || !attempt.released.isDone) {
            ledger.unknown(ticket)
            return
        }
        if (attempt.prepared.getNow(null) != null) {
            if (!ledger.closedExact(ticket)) ledger.unknown(ticket)
        } else if (!ledger.cancelBeforeBirth(ticket)) {
            ledger.unknown(ticket)
        }
    }
}
