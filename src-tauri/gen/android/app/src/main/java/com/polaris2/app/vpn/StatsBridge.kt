// 本仓自有文件（非移植）。
//
// 结构上参考 sing-box-for-android 的多 CommandClient 用法（每个屏一条自己的 client，各自
// addCommand 自己要的通道），但缓冲/游标/长轮询这一层是本仓独有的：上游的消费者是 Kotlin UI，
// 帧到手即渲染；本仓的消费者在 Rust 侧，帧必须跨语言交接，交接就要有「谁收到了哪一帧」的账。
package com.polaris2.app.vpn

import android.util.Log
import app.tauri.plugin.JSObject
import io.nekohasekai.libbox.CommandClient
import io.nekohasekai.libbox.CommandClientHandler
import io.nekohasekai.libbox.CommandClientOptions
import io.nekohasekai.libbox.Connection
import io.nekohasekai.libbox.ConnectionEvent
import io.nekohasekai.libbox.ConnectionEvents
import io.nekohasekai.libbox.Libbox
import io.nekohasekai.libbox.LogIterator
import io.nekohasekai.libbox.OutboundGroupItemIterator
import io.nekohasekai.libbox.OutboundGroupIterator
import io.nekohasekai.libbox.StatusMessage
import io.nekohasekai.libbox.StringIterator
import org.json.JSONArray
import java.util.ArrayDeque
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock

/**
 * Android 数据面桥的 Kotlin 半边：libbox `CommandClient` → 有界帧缓冲 → Rust。
 *
 * 设计与判据：`~/docs/polaris/design/polaris-android-dataplane-2026-09-04.md`。
 * 契约门：`scripts/check-android-bridge.mjs`（A6/A7/A8 三条守本文件）。
 *
 * # 为什么是「Rust 拉」而不是「Kotlin 推」
 *
 * 本仓在 Rust 侧已有一整套连接数据面（`runtime/stats/`：活动连接表、已结束历史环、四条 emit
 * 闸门、签名去重、订阅门）。Android 要的不是第二套，而是**把同样的帧喂进去**。
 * 帧从 Kotlin 到 Rust 只有两条路：
 *   - **推**：Kotlin 调 Rust 的 JNI 原生函数。要直接依赖 `jni` crate，而它在 lock 里同时有
 *     0.21.1 与 0.22.4 两个传递版本，挑错是链接期才炸（K4 的桥为此明写「零新增依赖是承重的」）。
 *   - **拉**：复用 K4 已定稿的 Tauri plugin 命令面。零新增依赖，且现成的契约门（A1/A2/A3）
 *     直接覆盖新命令。
 * 选后者。代价是「拉」天然要一次往返，故用**长轮询**把它压回推的延迟：没有帧时请求停在
 * [awaitDrain] 里等，帧一到当场结账。
 *
 * # 🔴 游标（`after`）是承重的，不是优化
 *
 * `PluginHandle::run_mobile_plugin_async` 的 future **被丢弃后**，Kotlin 侧迟到的回执会让
 * tauri 在 `tx.send(response).unwrap()` 上 panic（`tauri-2.11.5/src/plugin/mobile.rs:307`）。
 * Rust 侧因此绝不 `select!`/`timeout` racing 这些 future —— 但「请求可能白跑一趟」这件事仍要
 * 兜住（进程被杀、回执路径出错）。故 **drain 不破坏性**：帧只有在下一次请求带着更大的 `after`
 * 到达时才被裁掉。同一个 `after` 重放多少次，拿到的都是同一批帧。
 *
 * # 缓冲溢出 = 断流，不是丢帧
 *
 * 丢中间某几帧会静默毁掉两件事：CLOSED 事件丢了 ⇒ 已结束连接列表永远少几条；delta 丢了 ⇒
 * 逐连接累计字节从此偏低。两者都不报错、都看不出来。故溢出一律**整条流作废**（清空 + 标 closed），
 * Rust 收到 closed 就重订阅，而重订阅的首帧是 `reset=true` 全量表 —— 与桌面断流重连同一条语义。
 */
internal object StatsBridge {

    /**
     * `stats` topic 的供数流。
     *
     * 通道由 Rust 侧 `runtime/stats/source.rs` 的 `STATS_LIBBOX_COMMAND` 逐字对拍（契约门 A6）。
     */
    val statsStream = object : CommandStream("stats") {
        override fun subscribe(options: CommandClientOptions) {
            options.addCommand(Libbox.CommandStatus)
        }

        override fun writeStatus(message: StatusMessage) {
            enqueue(encodeStatus(message))
        }
    }

    /**
     * 四条连接需求（aggregate / topology / detail / closed）的**共同**供数流 —— 与桌面同构：
     * 一条上游流、一张连接表、四条各自节流的 emit。
     *
     * 通道由 Rust 侧 `CONNECTIONS_LIBBOX_COMMAND` 逐字对拍（契约门 A6）。
     */
    val connectionsStream = object : CommandStream("connections") {
        override fun subscribe(options: CommandClientOptions) {
            options.addCommand(Libbox.CommandConnections)
        }

        override fun writeConnectionEvents(events: ConnectionEvents?) {
            enqueue(encodeConnectionEvents(events ?: return))
        }
    }

    /**
     * 服务停机时收掉两条流。
     *
     * libbox 的 `disconnected()` 回调本来也会把流标成 closed，但那要等对端真的关掉 socket；
     * 显式收一次让「服务没了」与「订阅还挂着」不会有交叠窗口，也把两条 worker 线程放掉。
     */
    fun closeAll() {
        statsStream.suspendForStop()
        connectionsStream.suspendForStop()
    }

    /** 内核已经启动且命令服务可用；排在起核桥成功回执之前。 */
    fun activateAll() {
        statsStream.activate()
        connectionsStream.activate()
    }
}

/**
 * 一条 libbox 命令通道的订阅 + 有界帧缓冲 + 游标交接。
 *
 * `CommandClientHandler` 的其余回调在这里给出空实现：接口是全量的，而每条流只订自己那一个通道
 * （见 [subscribe]），没订的通道不会有回调进来。**已订通道的回调必须是真实现**，否则就是
 * 「订阅接上了、数据静默扔掉」—— 契约门 A7 正是守这件事（它按 [subscribe] 里的 `addCommand`
 * 推出该流必须实现哪个 `write*`，并要求那个方法体里出现 `enqueue(`）。
 */
internal abstract class CommandStream(private val name: String) : CommandClientHandler {

    private val lock = ReentrantLock()
    private val ready = lock.newCondition()

    /** `(seq, 帧)`；`seq` 单调递增，每次 [open] 归零（Rust 侧游标同时回到 0）。 */
    private val frames = ArrayDeque<Pair<Long, JSObject>>()
    private var nextSeq = 0L

    /** 非 null = 本条流已废（对端断开 / 溢出）。Rust 读到它就重订阅。 */
    private var closedReason: String? = null

    /** 服务停止后禁止旧 Rust running 快照在拆核窗口重开命令流。 */
    private var serviceReady = false
    private var serviceEpoch = 0L

    /** 只在 [worker] 那一条线程上读写（[open] / [close] / [closeClient] 都经它），故不需要同步。 */
    private var client: CommandClient? = null

    /** 单线程：`@Command` 一律不在主线程上阻塞（K4 的承重约束），长轮询也在这里等。 */
    private val worker: ExecutorService =
        Executors.newSingleThreadExecutor { r -> Thread(r, "polaris-$name-stream") }

    /** 本流要订哪个 libbox 通道。抽象而非空实现：删掉整个 override 是编译错误，不是静默不订。 */
    protected abstract fun subscribe(options: CommandClientOptions)

    /** 建流。`onDone(null)` = 成功；`onDone(msg)` = 失败（Rust 侧等一拍重试，同桌面建流失败腿）。 */
    fun open(onDone: (String?) -> Unit) {
        lock.lock()
        val requestEpoch = try { serviceEpoch } finally { lock.unlock() }
        worker.execute {
            val err = runCatching {
                // 排队的旧 open 不能先关掉新代 client 再报过期。
                lock.lock()
                try {
                    check(serviceReady && serviceEpoch == requestEpoch) { "内核命令服务未就绪或已换代" }
                } finally {
                    lock.unlock()
                }
                closeClient()
                lock.lock()
                try {
                    check(serviceReady && serviceEpoch == requestEpoch) { "内核命令服务未就绪或已换代" }
                    frames.clear()
                    nextSeq = 0L
                    closedReason = null
                } finally {
                    lock.unlock()
                }
                val options = CommandClientOptions()
                subscribe(options)
                options.statusInterval = STATUS_INTERVAL_NANOS
                val c = Libbox.newCommandClient(this@CommandStream, options)
                c.connect()
                client = c
                lock.lock()
                val stale = try { !serviceReady || serviceEpoch != requestEpoch }
                    finally { lock.unlock() }
                if (stale) {
                    closeClient()
                    error("内核命令服务已换代")
                }
            }.exceptionOrNull()
            if (err != null) Log.w(TAG, "[$name] 建流失败", err)
            onDone(err?.let { it.message ?: it.toString() })
        }
    }

    /** 收流。可在任意线程调用；正在长轮询的请求会被立刻唤醒并空手返回。 */
    fun close() {
        lock.lock()
        try {
            markClosed()
        } finally {
            lock.unlock()
        }
        worker.execute { closeClient() }
    }

    /** 停核屏障与收流同锁生效；随后任何排队的 open 都须拒绝。 */
    fun suspendForStop() {
        lock.lock()
        try {
            serviceReady = false
            serviceEpoch += 1
            markClosed()
        } finally {
            lock.unlock()
        }
        worker.execute { closeClient() }
    }

    fun activate() {
        lock.lock()
        try {
            serviceEpoch += 1
            serviceReady = true
        } finally {
            lock.unlock()
        }
    }

    private fun markClosed() {
        closedReason = closedReason ?: "已收流"
        frames.clear()
        ready.signalAll()
    }

    /** 长轮询取帧：`after` 之后的帧；没有就等到有（或 [MAX_WAIT_MS] 到）。 */
    fun poll(after: Long, onDone: (JSObject) -> Unit) {
        worker.execute { onDone(awaitDrain(after)) }
    }

    /**
     * 帧入缓冲。**已订通道的 `write*` 回调必须调到这里**（契约门 A7）。
     *
     * 溢出即断流：见类文档「缓冲溢出 = 断流，不是丢帧」。
     */
    protected fun enqueue(frame: JSObject) {
        lock.lock()
        try {
            if (closedReason != null) return
            if (frames.size >= CAPACITY) {
                frames.clear()
                closedReason = "帧缓冲溢出（$CAPACITY 帧未被取走）"
                Log.w(TAG, "[$name] $closedReason —— 整条流作废，等 Rust 重订阅")
            } else {
                nextSeq += 1
                frames.addLast(nextSeq to frame)
            }
            ready.signalAll()
        } finally {
            lock.unlock()
        }
    }

    private fun awaitDrain(after: Long): JSObject {
        lock.lock()
        try {
            // 上一批已被确认收到 ⇒ 现在才裁掉。裁剪前的重放是安全的（见类文档）。
            while (frames.isNotEmpty() && frames.peekFirst().first <= after) frames.removeFirst()
            var remaining = TimeUnit.MILLISECONDS.toNanos(MAX_WAIT_MS)
            while (frames.isEmpty() && closedReason == null && remaining > 0) {
                remaining = ready.awaitNanos(remaining)
            }
            val arr = JSONArray()
            var seq = after
            for ((s, frame) in frames) {
                if (arr.length() >= MAX_BATCH) break
                arr.put(frame)
                seq = s
            }
            return JSObject().apply {
                put("seq", seq)
                put("closed", closedReason != null)
                // org.json 的 put(key, null) 会**删掉**这个键 ⇒ 无因由时回包里没有 reason。
                put("reason", closedReason)
                put("frames", arr)
            }
        } finally {
            lock.unlock()
        }
    }

    private fun closeClient() {
        val c = client ?: return
        client = null
        runCatching { c.disconnect() }
    }

    // ── CommandClientHandler ────────────────────────────────────────────────────

    override fun connected() {
        Log.i(TAG, "[$name] 已连上内核命令服务")
    }

    override fun disconnected(message: String?) {
        lock.lock()
        try {
            closedReason = message?.takeIf { it.isNotBlank() } ?: "内核命令服务已断开"
            ready.signalAll()
        } finally {
            lock.unlock()
        }
    }

    // 以下通道本流不订（[subscribe] 里没有对应的 addCommand），接口是全量的，必须给出空实现。
    override fun clearLogs() {}

    override fun initializeClashMode(modeList: StringIterator?, currentMode: String?) {}

    override fun setDefaultLogLevel(level: Int) {}

    override fun updateClashMode(newMode: String?) {}

    override fun writeConnectionEvents(events: ConnectionEvents?) {}

    override fun writeGroups(groups: OutboundGroupIterator?) {}

    override fun writeLogs(messageList: LogIterator?) {}

    override fun writeOutbounds(outbounds: OutboundGroupItemIterator?) {}

    override fun writeStatus(message: StatusMessage) {}

    companion object {
        private const val TAG = "PolarisStatsBridge"

        /**
         * 帧缓冲上限。1s 一帧、Rust 侧长轮询即时取走 ⇒ 常态深度 0–1；256 是「Rust 侧停了四分钟」
         * 这个量级的余量，超过它说明对端不在了，继续攒只是把内存喂给一个没人读的队列。
         */
        private const val CAPACITY = 256

        /** 单次回包最多带几帧。够一次 4 分钟积压一口气交完，又不至于一包几 MB。 */
        private const val MAX_BATCH = 64

        /**
         * 长轮询的封顶等待。
         *
         * 必须**短于** Rust 侧对「桥没回应」的容忍，也必须有：没有它，空闲隧道上的那次请求会永远
         * 停在这里，而它对应的 oneshot 会一直挂在 tauri 的 `PENDING_PLUGIN_CALLS` 里。
         * 取 900ms 与 Rust 侧 `PARK_RECHECK_INTERVAL`（1s）同档：空转一次的代价是一次
         * 主线程 dispatch，和桌面 relay 每秒复核一次核状态是同一量级。
         */
        private const val MAX_WAIT_MS = 900L

        /** 与桌面 `STATS_STREAM_INTERVAL_NS` / `CONNECTIONS_STREAM_INTERVAL_NS` 同为 1s。 */
        private const val STATUS_INTERVAL_NANOS = 1_000_000_000L
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// 帧编码（libbox 值对象 → JSON）
//
// 🔴 **字段面与桌面逐字对齐，一个不多一个不少**：桌面 `runtime/stats/relay.rs` 的
// `daemon_conn_to_engine` 明写「刻意不加字段：这里映射哪些字段决定了 aggregate / detail 的输出」。
// 多发一个字段 = Android 上的显示内容与桌面不同；少发一个 = Rust 侧反序列化当场报错（各字段
// 均为必需键，没有 serde default 兜底）。两侧字段集由契约门 A8/A9 双向对拍。
// ══════════════════════════════════════════════════════════════════════════════

/** libbox `StatusMessage` → 桌面 `SingBoxStatus` 的九个字段（proto `message Status` 同形）。 */
private fun encodeStatus(m: StatusMessage): JSObject = JSObject().apply {
    put("memory", m.memory)
    put("goroutines", m.goroutines)
    put("connectionsIn", m.connectionsIn)
    put("connectionsOut", m.connectionsOut)
    put("trafficAvailable", m.trafficAvailable)
    put("uplink", m.uplink)
    put("downlink", m.downlink)
    put("uplinkTotal", m.uplinkTotal)
    put("downlinkTotal", m.downlinkTotal)
}

private fun encodeConnectionEvents(ev: ConnectionEvents): JSObject {
    val arr = JSONArray()
    val it = ev.iterator()
    while (it.hasNext()) arr.put(encodeConnectionEvent(it.next()))
    return JSObject().apply {
        put("reset", ev.reset)
        put("events", arr)
    }
}

/** `kind` 取 libbox 的 `ConnectionEventNew/Update/Closed`（0/1/2），与 proto enum 同值。 */
private fun encodeConnectionEvent(e: ConnectionEvent): JSObject = JSObject().apply {
    put("kind", e.type)
    put("id", e.id)
    // 只有 NEW（以及 reset 帧里的每一条）带 connection；UPDATE/CLOSED 只带 id + delta。
    e.connection?.let { put("connection", encodeConnection(it)) }
    put("uplinkDelta", e.uplinkDelta)
    put("downlinkDelta", e.downlinkDelta)
    put("closedAt", e.closedAt)
}

private fun encodeConnection(c: Connection): JSObject = JSObject().apply {
    put("id", c.id)
    put("inbound", c.inbound)
    put("inboundType", c.inboundType)
    put("network", c.network)
    put("source", c.source)
    put("destination", c.destination)
    put("domain", c.domain)
    put("createdAt", c.createdAt)
    put("closedAt", c.closedAt)
    put("uplinkTotal", c.uplinkTotal)
    put("downlinkTotal", c.downlinkTotal)
    put("rule", c.rule)
    put("chainList", collect(c.chain()))
    // 桌面只取 processPath（`SingBoxProcessInfo { process_path, ..Default::default() }`），
    // 这里跟着只取它。processInfo 在没开 find_process 的配置下是空的，两侧同样。
    put("processInfo", JSObject().apply { put("processPath", c.processInfo?.processPath ?: "") })
}

private fun collect(it: StringIterator?): JSONArray {
    val arr = JSONArray()
    if (it == null) return arr
    while (it.hasNext()) arr.put(it.next())
    return arr
}
