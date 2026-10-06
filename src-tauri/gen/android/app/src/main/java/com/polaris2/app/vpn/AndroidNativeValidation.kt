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

    internal fun enqueueWarm(ledger: AndroidNativeAdmission, owner: AndroidNativeAdmission.Ticket,
        queue: (() -> Unit) -> Unit, action: (AndroidNativeAdmission.Ticket) -> Unit) {
        val child = ledger.reserveWarmValidation(owner)
        try { queue { action(child) } }
        catch (failure: Throwable) { ledger.cancelBeforeBirth(child); throw failure }
    }

    fun check(ticket: AndroidNativeAdmission.Ticket, config: String) {
        AndroidNativeAdmissionGate.ledger.checkWarmValidationConfig(ticket, config)
        run(AndroidNativeAdmissionGate.ledger, ticket,
            setup = { PolarisApplication.ensureSetup() },
            nativeCheck = {
                val method = runCatching { Libbox::class.java.getMethod("checkConfigWithResult",
                    String::class.java, String::class.java, java.lang.Long.TYPE) }.getOrNull()
                if (method == null) {
                    // Legacy AAR retains its ordinary validation behavior, without scoped evidence.
                    Libbox.checkConfig(config)
                } else {
                    val result = try { checkNotNull(method.invoke(null, config, ticket.id, 20_000L)) }
                    catch (failure: java.lang.reflect.InvocationTargetException) { throw failure.targetException }
                    runCatching {
                        val scope = AndroidTailscaleValidationScope.capture(ticket.id, config, result)
                        AndroidNativeAdmissionGate.ledger.attachValidation(ticket, scope)
                    }
                    fun field(name: String) = result.javaClass.getMethod(name).invoke(result) as String
                    if (field("getValidation") != "Accepted") throw IllegalArgumentException(field("getValidationError"))
                }
            })
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
            throw ledger.admissionRejection()
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
