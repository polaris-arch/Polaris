package com.polaris2.app.vpn

/** Production callback sequence; the Service supplies exact-target, Host, JNI and notice adapters. */
internal class AndroidNativeMainControls(private val ledger: AndroidNativeAdmission) {
    fun rejectUnbound(
        kind: AndroidNativeAdmission.Kind,
        logFailure: (Throwable) -> Unit,
    ): AndroidNativeAdmission.Ticket? = try {
        AndroidNativeControlOperation.rejectUnbound(ledger, kind)
    } catch (failure: Throwable) {
        runCatching { logFailure(failure) }
        null
    }

    fun <Server : Any> reload(
        attempt: MainKernelAttempt<Server>,
        target: () -> Server?,
        allowed: (Server) -> Boolean,
        loadConfig: () -> String,
        preflight: (String) -> Unit,
        withHosts: (Server, String, () -> Unit) -> Unit,
        nativeReload: (Server, String) -> Unit,
        setError: (Server, Throwable) -> Unit,
        reconnectNotice: () -> Unit,
        logFailure: (Throwable) -> Unit,
    ): AndroidNativeAdmission.Ticket? {
        var firstFailure: Throwable? = null
        fun logFirst(failure: Throwable) {
            if (firstFailure == null) {
                firstFailure = failure
                runCatching { logFailure(failure) }
            }
        }
        // Reserve at callback ingress, before even the exact target lookup or config I/O.
        val operation = try {
            AndroidNativeControlOperation.reserve(ledger, AndroidNativeAdmission.Kind.TargetlessReload)
        } catch (failure: Throwable) { logFirst(failure); return null }
        val server = try { target() } catch (failure: Throwable) {
            operation.cancelBeforeDispatch()
            logFirst(failure)
            return operation.ticket
        }
        if (server == null) { operation.cancelBeforeDispatch(); return operation.ticket }
        try {
            operation.runExact(attempt, { allowed(server) }) { boundary ->
                try {
                    val config = loadConfig()
                    try { preflight(config) }
                    catch (_: DualModeEndpointTombstone.ReloadRequiresReconnect) {
                        // Only pure preflight may decline this way. Preserve the live core.
                        try { boundary.ifCurrent(reconnectNotice) }
                        catch (failure: Throwable) { logFirst(failure) }
                        return@runExact
                    }
                    boundary.requireBirthAllowed()
                    // These adapters retain their own original-owner cleanup and claim order.
                    // They do not assert that a main-core construction has entered JNI.
                    withHosts(server, config) {
                        boundary.construct { nativeReload(server, config) }
                    }
                } catch (failure: Throwable) {
                    boundary.recordFailure(failure)
                    logFirst(failure)
                    boundary.reportFailure(failure) { setError(server, failure) }
                }
            }
        } catch (failure: Throwable) { logFirst(failure) }
        return operation.ticket
    }
}
