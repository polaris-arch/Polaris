package com.polaris2.app.vpn

/** The production guardian consumes timestamped SDK observations; it never waits for SDK work. */
internal object DebugBatchGuard {
    fun platformReady(now: Long, sampledAt: Long?, allowed: Boolean?): Boolean =
        sampledAt != null && allowed == true && now >= sampledAt && now - sampledAt < 2000

    fun abortReason(now: Long, createdAt: Long, deadline: Long, controllerLeaseUntil: Long,
                    bindingCurrent: Boolean, sampledAt: Long?, allowed: Boolean?): String? {
        if (now < createdAt || now >= deadline || now >= controllerLeaseUntil || !bindingCurrent)
            return "bounded-guard-abort"
        val last = sampledAt ?: createdAt
        if (allowed == false || now < last || now - last >= 2000) return "platform-signal-unavailable"
        return null
    }
}
