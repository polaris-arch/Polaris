// 移植自 sing-box-for-android，文件：app/src/main/java/io/nekohasekai/sfa/bg/LocalResolver.kt
// Copyright (C) 2022 by nekohasekai <contact-sagernet@sekai.icu>
// 该项目按 GNU General Public License v3（或更新版本）分发，本文件因此继承 GPLv3。
// 与上游的差异：上游用 kotlinx-coroutines 的 suspendCoroutine + runBlocking 把 DnsResolver 的
// 异步回调转成同步；本仓不引 coroutines，改用 CountDownLatch（语义相同：Go 侧是同步调用，
// 必须在本方法返回前把结果写进 ExchangeContext）。

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
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
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
    // rawQuery（收发 DNS 报文原文）是 API 29 才有的；29 以下只能退到 InetAddress 级别的 lookup()。
    override fun raw(): Boolean = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q

    // 不用 direct executor：DnsResolver 在哪个线程回调是它的实现细节，万一它同步回调到调用线程，
    // 下面的 latch.await() 就会等一个永远不会 countDown 的锁。给一份自己的线程池把这条排除掉。
    private val executor: Executor = Executors.newCachedThreadPool()

    @RequiresApi(Build.VERSION_CODES.Q)
    override fun exchange(ctx: ExchangeContext, message: ByteArray) {
        val network = currentNetwork() ?: error("android: 没有可用的默认网络")
        val latch = CountDownLatch(1)
        val signal = CancellationSignal()
        var failure: Exception? = null
        ctx.onCancel {
            signal.cancel()
            latch.countDown()
        }
        DnsResolver.getInstance().rawQuery(
            network,
            message,
            DnsResolver.FLAG_NO_RETRY,
            executor,
            signal,
            object : DnsResolver.Callback<ByteArray> {
                override fun onAnswer(answer: ByteArray, rcode: Int) {
                    if (rcode == 0) ctx.rawSuccess(answer) else ctx.errorCode(rcode)
                    latch.countDown()
                }

                override fun onError(error: DnsResolver.DnsException) {
                    // errno 要原样回传：内核靠 ENETUNREACH / EPERM 之类区分「网络没了」与「查询失败」，
                    // 一律翻成异常会让它把可重试的错误当成永久失败。
                    when (val cause = error.cause) {
                        is ErrnoException -> ctx.errnoCode(cause.errno)
                        else -> failure = error
                    }
                    latch.countDown()
                }
            },
        )
        latch.await()
        failure?.let { throw it }
    }

    override fun lookup(ctx: ExchangeContext, network: String, domain: String) {
        val defaultNetwork = currentNetwork() ?: error("android: 没有可用的默认网络")
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            val answer = try {
                defaultNetwork.getAllByName(domain)
            } catch (e: UnknownHostException) {
                ctx.errorCode(RCODE_NXDOMAIN)
                return
            }
            ctx.success(answer.mapNotNull { it.hostAddress }.joinToString("\n"))
            return
        }
        lookupQ(ctx, network, domain, defaultNetwork)
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private fun lookupQ(ctx: ExchangeContext, network: String, domain: String, defaultNetwork: android.net.Network) {
        val latch = CountDownLatch(1)
        val signal = CancellationSignal()
        var failure: Exception? = null
        ctx.onCancel {
            signal.cancel()
            latch.countDown()
        }
        val callback = object : DnsResolver.Callback<Collection<InetAddress>> {
            override fun onAnswer(answer: Collection<InetAddress>, rcode: Int) {
                if (rcode == 0) {
                    ctx.success(answer.mapNotNull { it.hostAddress }.joinToString("\n"))
                } else {
                    ctx.errorCode(rcode)
                }
                latch.countDown()
            }

            override fun onError(error: DnsResolver.DnsException) {
                when (val cause = error.cause) {
                    is ErrnoException -> ctx.errnoCode(cause.errno)
                    else -> failure = error
                }
                latch.countDown()
            }
        }
        // network 形如 "tcp4" / "udp6" / "tcp"：内核用它表达「只要 A / 只要 AAAA / 都要」。
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
        latch.await()
        failure?.let { throw it }
    }

    private val RCODE_NXDOMAIN = 3
}
