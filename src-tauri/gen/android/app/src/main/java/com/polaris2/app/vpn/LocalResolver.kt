// 移植自 sing-box-for-android，文件：app/src/main/java/io/nekohasekai/sfa/bg/LocalResolver.kt
// Copyright (C) 2022 by nekohasekai <contact-sagernet@sekai.icu>
// 该项目按 GNU General Public License v3（或更新版本）分发，本文件因此继承 GPLv3。
// 与上游的差异：上游用 kotlinx-coroutines 的 suspendCoroutine + runBlocking 把 DnsResolver 的
// 异步回调转成同步；本仓不引 coroutines，改用可撤销 mailbox。Go 侧是同步调用，
// 结果仅由原调用线程在方法返回前写进 ExchangeContext。

package com.polaris2.app.vpn

import android.net.DnsResolver
import android.os.Build
import android.os.CancellationSignal
import android.system.ErrnoException
import androidx.annotation.RequiresApi
import io.nekohasekai.libbox.ExchangeContext
import io.nekohasekai.libbox.LocalDNSTransport
import java.net.InetAddress
import java.net.UnknownHostException
import java.util.concurrent.Executors

/**
 * 把内核的「系统 DNS」查询接到 Android 框架上。
 *
 * 为什么必须由平台侧做：应用进程读不到 /etc/resolv.conf，也没有 netd 的直连通道；更要紧的是查询
 * **必须绑到底层物理网络**（`DefaultNetworkMonitor.defaultNetwork`），否则 DNS 包会走进我们自己
 * 刚建起来的 tun，而 tun 的上游解析又依赖这次查询 —— 死锁。
 */
object LocalResolver : LocalDNSTransport by NetworkLocalResolver({ DefaultNetworkMonitor.defaultNetwork })

/** A login instance supplies its own physical-network monitor; the main resolver keeps its existing supplier. */
internal class NetworkLocalResolver(private val currentNetwork: () -> android.net.Network?) : LocalDNSTransport {
    private val lifecycle = TransientResolverLifecycle()
    override fun raw(): Boolean = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q

    // A queried executor must continue accepting SDK result/fd cleanup tasks, including after local close.
    private val workers = Executors.newCachedThreadPool()
    private val executor = lifecycle.sdkExecutor(workers)

    fun beginClose() = lifecycle.beginClose()

    /** Only an unused resolver has a positive no-callback fact. Queried instances remain Unknown. */
    fun closeUnused() {
        lifecycle.closeUnused()
        workers.shutdown()
        if (!workers.isTerminated) throw TransientResolverLifecycle.CleanupUnknown()
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    override fun exchange(ctx: ExchangeContext, message: ByteArray) = withQuery(ctx) { query ->
        val network = currentNetwork() ?: error("android: 没有可用的默认网络")
        val signal = CancellationSignal()
        query.installCancellation { signal.cancel() }
        if (query.enterSdkSubmission()) {
            DnsResolver.getInstance().rawQuery(network, message, DnsResolver.FLAG_NO_RETRY,
                executor, signal, RawResolverCallback(query))
        }
        query.awaitAndDeliver { it.deliver(ctx) }
    }

    override fun lookup(ctx: ExchangeContext, network: String, domain: String) = withQuery(ctx) { query ->
        val defaultNetwork = currentNetwork() ?: error("android: 没有可用的默认网络")
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            // This SDK API is synchronous and has no cancellation handle. Keep the call counted until return.
            if (query.canStartBlockingLookup()) {
                val result = try {
                    ResolverResult.Addresses(defaultNetwork.getAllByName(domain)
                        .mapNotNull { it.hostAddress }.joinToString("\n"))
                } catch (_: UnknownHostException) { ResolverResult.ErrorCode(RCODE_NXDOMAIN) }
                query.publish(result)
            }
        } else {
            lookupQ(query, network, domain, defaultNetwork)
        }
        query.awaitAndDeliver { it.deliver(ctx) }
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private fun lookupQ(query: TransientResolverLifecycle.Query<ResolverResult>, network: String,
                        domain: String, defaultNetwork: android.net.Network) {
        val signal = CancellationSignal()
        query.installCancellation { signal.cancel() }
        if (!query.enterSdkSubmission()) return
        val callback = AddressResolverCallback(query)
        // network is tcp4/udp6/tcp: the core requests A, AAAA, or both.
        val type = when {
            network.endsWith("4") -> DnsResolver.TYPE_A
            network.endsWith("6") -> DnsResolver.TYPE_AAAA
            else -> null
        }
        val resolver = DnsResolver.getInstance()
        if (type != null) {
            resolver.query(defaultNetwork, domain, type, DnsResolver.FLAG_NO_RETRY, executor, signal, callback)
        } else {
            resolver.query(defaultNetwork, domain, DnsResolver.FLAG_NO_RETRY, executor, signal, callback)
        }
    }

    private fun withQuery(ctx: ExchangeContext, action: (TransientResolverLifecycle.Query<ResolverResult>) -> Unit) {
        val query = lifecycle.enterQuery<ResolverResult>()
        val hook = TransientResolverCancelHook(query)
        try {
            // This registration captures only a clearable local holder, never ctx or this resolver.
            ctx.onCancel { hook.cancel() }
            action(query)
        } catch (error: Throwable) {
            query.cancel()
            throw error
        } finally {
            // Does not prove physical JNI proxy reclamation or unregister Go's current cancellation goroutine.
            hook.clear()
            query.returned()
        }
    }

    private val RCODE_NXDOMAIN = 3
}

private sealed class ResolverResult {
    data class Raw(val bytes: ByteArray) : ResolverResult()
    data class Addresses(val value: String) : ResolverResult()
    data class ErrorCode(val code: Int) : ResolverResult()
    data class Errno(val code: Int) : ResolverResult()
    data class Failure(val error: Exception) : ResolverResult()

    // Only called on the original exchange/lookup stack, after its delivery permit.
    fun deliver(ctx: ExchangeContext) = when (this) {
        is Raw -> ctx.rawSuccess(bytes)
        is Addresses -> ctx.success(value)
        is ErrorCode -> ctx.errorCode(code)
        is Errno -> ctx.errnoCode(code)
        is Failure -> throw error
    }
}

private fun resolverError(error: DnsResolver.DnsException): ResolverResult = when (val cause = error.cause) {
    is ErrnoException -> ResolverResult.Errno(cause.errno)
    else -> ResolverResult.Failure(error)
}

// These top-level callback objects retain only the pure Kotlin mailbox, with no native context or network supplier.
private class RawResolverCallback(private val query: TransientResolverLifecycle.Query<ResolverResult>) :
    DnsResolver.Callback<ByteArray> {
    override fun onAnswer(answer: ByteArray, rcode: Int) {
        query.publish(if (rcode == 0) ResolverResult.Raw(answer.copyOf()) else ResolverResult.ErrorCode(rcode))
    }
    override fun onError(error: DnsResolver.DnsException) { query.publish(resolverError(error)) }
}

private class AddressResolverCallback(private val query: TransientResolverLifecycle.Query<ResolverResult>) :
    DnsResolver.Callback<Collection<InetAddress>> {
    override fun onAnswer(answer: Collection<InetAddress>, rcode: Int) {
        query.publish(if (rcode == 0) ResolverResult.Addresses(answer.mapNotNull { it.hostAddress }.joinToString("\n"))
            else ResolverResult.ErrorCode(rcode))
    }
    override fun onError(error: DnsResolver.DnsException) { query.publish(resolverError(error)) }
}
