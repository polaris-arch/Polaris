// 本仓自有文件（非移植）。
package com.polaris2.app.vpn

import android.content.Context
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.util.Log
import java.io.File
import java.security.MessageDigest

/**
 * **不经桥的起核**（系统发起：always-on VPN / 开机接收器 / 进程被回收后系统重拉）的配置来源与准入。
 *
 * # 缺陷来由
 *
 * 配置此前只存在于进程内存（[`VpnBridge`] 的 `config`，唯一入口是 Rust 经 `start` 命令交来）。
 * 系统以 `android.net.VpnService` 意图拉起服务时根本没有桥调用 ⇒ `BoxService.startKernel` 拿到
 * `null` ⇒ 抛错自停。设置页却在引导用户去开「始终开启的 VPN」—— 那条路**一定**失败。
 *
 * # 真值单一：配置字节只有 Rust 落的那一份
 *
 * Rust 起核时 `std::fs::write(config_path, &json)` 与交给桥的 `configContent` 是**同一个 `json`
 * 变量**（`runtime/proxy/startup.rs` 的 `generate_and_gate_with_runtime_bindings`）。`config_path` =
 * `app_config_dir()/polaris/singbox-runtime.json`，而 Tauri Android 的 `app_config_dir()` 就是
 * `Context.getDataDir()`（`tauri-2.11.5/mobile/android/.../PathPlugin.kt` 的 `getConfigDir`）。
 * 故本类**不另存配置**，只读那一份；两段路径字面量由 `scripts/check-android-bridge.mjs` 的 A13
 * 与 Rust 侧逐字对拍。
 *
 * # 本类自己只存一样东西：「最近一次**经桥成功**起核的配置摘要」（sha256）
 *
 * 放 `noBackupFilesDir`（不进自动备份、不在外部存储）。它回答两个问题：
 *  · **准入**：它在 ⇔ 用户最后一次的意图是「连着」。用户主动断开（应用内断开 / 通知栏「断开」/
 *    系统里撤销授权）即删 ⇒ 之后系统再拉起服务也只会留一行日志然后自停，不会违背用户意图重连。
 *  · **身份**：Rust 之后可能为一次**没起成功**的尝试覆盖了盘上文件（生成成功、闸门拒收 / 起核失败）。
 *    摘要对不上 ⇒ 盘上已不是「最近一次成功起核的那份」⇒ 拒绝，而不是拿一份没人验证过的配置起核。
 *
 * # 开机自动连接开关（`autoStart` 的 Android 腿）
 *
 * 同目录另一个标记文件：存在 ⇔ 用户在设置 → 通用里打开了「开机自动连接」。它是 Rust
 * `auto_start_set` 经桥写进来的（与桌面 `auto_start_set` 写 OS launch agent 同构 —— 真值住在
 * 执行开机动作的那一方，因为开机那一刻 Rust 根本没有在跑）。消费方只有 [`BootReceiver`]。
 */
internal object SystemStart {
    private const val TAG = "PolarisSystemStart"

    /** Rust `lib.rs` setup 里 `app_config_dir().join("polaris")` 的那一段。A13 逐字对拍。 */
    private const val RUST_CONFIG_SUBDIR = "polaris"

    /** Rust `ProxyRuntime::runtime_config_path` 的文件名。A13 逐字对拍。 */
    private const val RUST_RUNTIME_CONFIG_FILE = "singbox-runtime.json"

    /** Rust polaris_store::mesh_guard::REQUIRED_MARKER_FILE; present from Preparing onward. */
    private const val MANAGED_MARKER_FILE = "mesh-route-state.required"

    /** 最近一次经桥成功起核的配置摘要（存在 ⇔ 允许不经桥起核）。 */
    private const val STARTED_DIGEST_FILE = "system-start.sha256"

    /** 「开机自动连接」开关（存在 ⇔ 开）。 */
    private const val BOOT_AUTO_CONNECT_FILE = "boot-auto-connect"
    private var disconnectGeneration = 0L

    @Synchronized
    fun generation(): Long = disconnectGeneration

    private fun runtimeConfig(context: Context): File =
        File(File(context.dataDir, RUST_CONFIG_SUBDIR), RUST_RUNTIME_CONFIG_FILE)

    private fun managedMarker(context: Context): File =
        File(File(context.dataDir, RUST_CONFIG_SUBDIR), MANAGED_MARKER_FILE)

    /** Only ENOENT proves legacy admission. Any other stat failure remains unknown and blocks. */
    fun requireLegacyAllowed(context: Context) {
        LegacySystemStartFence.requireOpen()
        val marker = managedMarker(context)
        try {
            Os.stat(marker.path)
            error("android: 受管路由已开始迁移，旧摘要自启被拒")
        } catch (error: ErrnoException) {
            if (error.errno != OsConstants.ENOENT) {
                throw IllegalStateException("android: 无法核实受管路由标记，旧摘要自启被拒", error)
            }
        }
        LegacySystemStartFence.requireOpen()
    }

    private fun digestFile(context: Context): File = File(context.noBackupFilesDir, STARTED_DIGEST_FILE)

    private fun bootFlagFile(context: Context): File = File(context.noBackupFilesDir, BOOT_AUTO_CONNECT_FILE)

    internal fun sha256(bytes: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

    /**
     * 经桥起核**成功之后**调：记下这份配置的摘要。
     *
     * 摘要取自**盘上那份**，且先与桥交来的字符串比一次：两者不等 ⇒ Kotlin 与 Rust 对「配置文件在哪」
     * 的理解已经分叉（路径漂移），此时写摘要只会让将来的系统起核拿到错的东西。故不写、删旧的、
     * 并**当场**报错日志 —— 这一形在「用户主动连接」这条最常走的路上就会自曝，不必等到开机那一刻。
     */
    @Synchronized
    fun remember(context: Context, bridgeConfig: String, expectedGeneration: Long, allowed: () -> Boolean) {
        // This check shares forget's monitor with the write. A late success from an
        // attempt that predates user disconnect can never restore auto-start admission.
        if (disconnectGeneration != expectedGeneration || !allowed()) return
        requireLegacyAllowed(context)
        val file = runtimeConfig(context)
        val onDisk = runCatching { file.readBytes() }.getOrNull()
        val expected = sha256(bridgeConfig.toByteArray(Charsets.UTF_8))
        if (onDisk == null || sha256(onDisk) != expected) {
            digestFile(context).delete()
            Log.e(
                TAG,
                "盘上配置 ${file.path} 与桥交来的不是同一份（" +
                    (if (onDisk == null) "文件不存在" else "摘要不等") +
                    "）—— 系统发起的起核将不可用",
            )
            return
        }
        writeAtomically(digestFile(context), expected)
        // A marker published while the digest was being written cannot leave old admission behind.
        try {
            requireLegacyAllowed(context)
        } catch (error: Throwable) {
            digestFile(context).delete()
            throw error
        }
        Log.i(TAG, "已记下本次起核配置，系统发起的起核可用")
    }

    /** 用户主动断开（或其它「不应再自启」的时刻）：删摘要。幂等。 */
    @Synchronized
    fun forget(context: Context, why: String) {
        disconnectGeneration++
        if (digestFile(context).delete()) Log.i(TAG, "已撤销系统发起起核的准入：$why")
    }

    /**
     * 不经桥起核时取配置。失败一律抛带原因的异常（调用方把原话记日志 / 交给结账），**不返回 null** ——
     * 「为什么没起」在开机场景下没有任何界面能展示，logcat 里那一行是唯一的证据。
     */
    @Synchronized
    fun load(context: Context): String {
        requireLegacyAllowed(context)
        val digest = runCatching { digestFile(context).readText().trim() }.getOrNull()
            ?: error("android: 不经桥起核被拒：用户上次主动断开，或从未成功起核过")
        val file = runtimeConfig(context)
        val bytes = runCatching { file.readBytes() }.getOrNull()
            ?: error("android: 不经桥起核被拒：找不到 ${file.path}")
        if (sha256(bytes) != digest) {
            error("android: 不经桥起核被拒：盘上配置已被一次未成功的起核尝试覆盖，不是最近一次成功起核的那份")
        }
        requireLegacyAllowed(context)
        return String(bytes, Charsets.UTF_8)
    }

    fun setBootAutoConnect(context: Context, enabled: Boolean) {
        val flag = bootFlagFile(context)
        if (enabled) {
            writeAtomically(flag, "1")
        } else if (flag.exists() && !flag.delete()) {
            error("android: 删除开机自动连接标记失败：${flag.path}")
        }
    }

    fun bootAutoConnect(context: Context): Boolean = bootFlagFile(context).exists()

    /** 先写临时文件再 rename：写到一半断电不会留下一个「存在但内容残缺」的摘要。 */
    private fun writeAtomically(target: File, content: String) {
        target.parentFile?.mkdirs()
        val tmp = File(target.parentFile, "${target.name}.tmp")
        tmp.writeText(content)
        if (!tmp.renameTo(target)) {
            tmp.delete()
            error("android: 写 ${target.path} 失败")
        }
    }
}
