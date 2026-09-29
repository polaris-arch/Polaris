package com.polaris2.app.vpn

import io.nekohasekai.libbox.Libbox

/** Every checkConfig call must keep its ticket until JNI actually returns. */
internal object AndroidNativeValidation {
    val capabilities = setOf(AndroidNativeProducer.ValidationCheckConfig)
    fun reserve(): AndroidNativeAdmission.Ticket =
        AndroidNativeAdmissionGate.ledger.reserveOperation(AndroidNativeAdmission.Kind.CheckConfig)

    fun cancelBeforeBirth(ticket: AndroidNativeAdmission.Ticket) {
        require(ticket.kind == AndroidNativeAdmission.Kind.CheckConfig)
        AndroidNativeAdmissionGate.ledger.cancelBeforeBirth(ticket)
    }

    fun check(config: String) = check(reserve(), config)

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
