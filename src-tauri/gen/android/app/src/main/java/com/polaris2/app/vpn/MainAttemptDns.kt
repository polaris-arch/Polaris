package com.polaris2.app.vpn

import io.nekohasekai.libbox.LocalDNSTransport
import io.nekohasekai.libbox.PlatformInterface
import java.util.concurrent.atomic.AtomicReference

/** Immutable attempt resource: the getter and both close hooks refer to the same resolver. */
internal class MainAttemptDns(
    val transport: LocalDNSTransport,
    private val fence: () -> Unit,
    private val disposeUnused: () -> Unit,
) {
    private val failure = AtomicReference<Throwable?>()
    val fenceFailure: Throwable? get() = failure.get()
    /** Captures this immutable resource; a late getter cannot look up a successor. */
    fun bindPlatform(delegate: PlatformInterface): PlatformInterface =
        object : PlatformInterface by delegate {
            override fun localDNSTransport(): LocalDNSTransport = transport
        }

    // Production fence only seals metadata and enqueues cancellation. It must not
    // run SDK cancellation inline or wait, including under BoxService's state lock.
    fun beginClose() {
        // A failed early fence must not strand the original owner before its
        // native close is launched. Record uncertainty without changing that result.
        try { fence() }
        catch (error: Throwable) { failure.compareAndSet(null, error) }
    }

    // Only a rejected, never-exposed resource uses this. It is not main close proof.
    fun closeUnused() = disposeUnused()

    companion object {
        /** Construction creates no SDK query and never touches the singleton resolver. */
        fun create(currentNetwork: () -> android.net.Network? = { DefaultNetworkMonitor.defaultNetwork }): MainAttemptDns {
            val resolver = NetworkLocalResolver(currentNetwork)
            return MainAttemptDns(resolver, resolver::beginClose, resolver::closeUnused)
        }
    }
}
