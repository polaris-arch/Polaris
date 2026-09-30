package com.polaris2.app.vpn

/** One login attempt keeps its original ticket even after operational disposal. */
internal class TransientLoginNativeOwner private constructor(
    private val ledger: AndroidNativeAdmission,
    val ticket: AndroidNativeAdmission.Ticket,
) {
    private val gate = Any()
    @Volatile private var revoked = false
    val cancelled: Boolean get() = revoked || ledger.state(ticket) == AndroidNativeAdmission.State.CancelledBeforeBirth

    /** Both owner and independent validation reservations exist before dispatch. */
    fun enqueue(queue: (() -> Unit) -> Unit, action: (AndroidNativeAdmission.Ticket) -> Unit) {
        try { AndroidNativeValidation.enqueue(ledger, queue, action) }
        catch (failure: Throwable) { cancel(); throw failure }
    }

    fun enterBirth(): Boolean = synchronized(gate) {
        if (revoked || !ledger.enterBirth(ticket)) {
            ledger.cancelBeforeBirth(ticket)
            false
        } else true
    }

    fun cancel() = synchronized(gate) {
        revoked = true
        ledger.retireOwner(AndroidNativeAdmission.Kind.Login, ticket.logicalId)
    }

    /** Host holds ownershipLock until this returns or records construction failure. */
    fun <T> construct(action: () -> T): T {
        if (!enterBirth()) throw ledger.admissionRejection()
        try {
            val result = action()
            check(!cancelled) { "Android 登录请求已取消" }
            return result
        } catch (failure: Throwable) {
            constructionFailed()
            throw failure
        }
    }

    /** Recorded before the host releases its construction/ownership lock. */
    fun constructionFailed() = synchronized(gate) {
        if (ledger.state(ticket) == AndroidNativeAdmission.State.BirthEntered) ledger.unknown(ticket)
    }

    /** This slice does not yet prove worker/network/DNS/cache or TS release. */
    fun closedWithoutProof() = synchronized(gate) {
        revoked = true
        if (!ledger.cancelBeforeBirth(ticket)) ledger.unknown(ticket)
    }

    companion object {
        val capabilities = emptySet<AndroidNativeProducer>()

        fun reserve(ledger: AndroidNativeAdmission, id: String) =
            TransientLoginNativeOwner(ledger, ledger.reserveOwner(AndroidNativeAdmission.Kind.Login, id))

        /** Also consumes an ID whose Start has not reached the host yet. */
        fun retireBeforeStart(ledger: AndroidNativeAdmission, id: String) =
            ledger.retireOwner(AndroidNativeAdmission.Kind.Login, id)
    }
}
