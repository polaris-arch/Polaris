package com.polaris2.app.vpn

import io.nekohasekai.libbox.Libbox

/** Every checkConfig call must keep its ticket until JNI actually returns. */
internal object AndroidNativeValidation {
    val capabilities = setOf(AndroidNativeProducer.ValidationCheckConfig)
    fun cancelBeforeBirth(ticket: AndroidNativeAdmission.Ticket) {
        require(ticket.kind == AndroidNativeAdmission.Kind.CheckConfig)
        AndroidNativeAdmissionGate.ledger.cancelBeforeBirth(ticket)
    }

    /** All three ingress paths reserve before their first asynchronous queue. */
    fun enqueue(queue: (() -> Unit) -> Unit, action: (AndroidNativeAdmission.Ticket) -> Unit) =
        enqueue(AndroidNativeAdmissionGate.ledger, queue, action)

    internal fun enqueue(ledger: AndroidNativeAdmission, queue: (() -> Unit) -> Unit,
        action: (AndroidNativeAdmission.Ticket) -> Unit) {
        val ticket = ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)
        try { queue { action(ticket) } }
        catch (failure: Throwable) {
            ledger.cancelBeforeBirth(ticket)
            throw failure
        }
    }

    fun check(ticket: AndroidNativeAdmission.Ticket, config: String) {
        run(AndroidNativeAdmissionGate.ledger, ticket,
            setup = { PolarisApplication.ensureSetup() },
            nativeCheck = { Libbox.checkConfig(config) })
    }

    internal fun run(
        ledger: AndroidNativeAdmission,
        ticket: AndroidNativeAdmission.Ticket,
        setup: () -> Unit,
        nativeCheck: () -> Unit,
    ) {
        require(ticket.kind == AndroidNativeAdmission.Kind.CheckConfig)
        if (!ledger.enterBirth(ticket)) {
            ledger.cancelBeforeBirth(ticket)
            throw AndroidNativeAdmission.AdmissionClosed()
        }
        try {
            setup()
            nativeCheck()
        } finally {
            // Current Go checkConfig cannot prove cleanup of unstarted or partially
            // constructed objects, even when its JNI call returns successfully.
            ledger.validationCleanupUnknown(ticket)
        }
    }
}
