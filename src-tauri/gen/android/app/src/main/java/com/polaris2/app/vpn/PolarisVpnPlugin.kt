// 本仓自有文件（非移植）。
package com.polaris2.app.vpn

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.content.pm.ResolveInfo
import android.net.Uri
import android.net.VpnService
import android.os.Build
import android.provider.Settings
import android.util.Log
import androidx.activity.result.ActivityResult
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.polaris2.app.PolarisBackupAgent
import com.polaris2.app.BuildConfig
import io.nekohasekai.libbox.Libbox
import java.io.File
import java.util.Locale
import java.util.UUID
import org.json.JSONArray

@InvokeArg
class DebugReportArgs {
    lateinit var report: String
}

@InvokeArg
class MainStartArgs {
    lateinit var configContent: String
    lateinit var runId: String
    lateinit var configDigest: String
    var claim: String? = null
}

internal data class MainStartRequest(
    val configContent: String,
    val runId: String,
    val configDigest: String,
    val claim: String?,
)

@InvokeArg
class CheckArgs {
    lateinit var configContent: String
}

@InvokeArg
class TransientLoginStartArgs {
    lateinit var instanceId: String
    lateinit var configContent: String
}

@InvokeArg
class TransientLoginInstanceArgs {
    lateinit var instanceId: String
}

@InvokeArg
class TransientSpeedtestStartArgs {
    lateinit var instanceId: String
    lateinit var configContent: String
}

@InvokeArg
class TransientSpeedtestInstanceArgs {
    lateinit var instanceId: String
}

@InvokeArg
class BootAutoConnectArgs {
    var enabled: Boolean = false
}

@InvokeArg
class SystemBackupArgs {
    var enabled: Boolean = false
}

@InvokeArg
class InstallApkArgs {
    /**
     * 已下载好的 APK **绝对路径**（Rust 侧 `update_download` 落到 `app_cache_dir()/updates/` 的那一份）。
     *
     * Kotlin 侧不下载、不校验摘要、不改名 —— 完整性由 Rust 那条腿的 sha256 / fileSize 三级判据负责
     * （`commands/updater/app_update.rs`）。这里只做一件事：确认这个路径落在应用私有 cache 目录里，
     * 然后把它交给系统安装器。多写一份下载/校验就是第二份真值。
     */
    lateinit var apkPath: String
}

@InvokeArg
class PollArgs {
    /**
     * 上一批**已被 Rust 收下**的最后一帧序号；`0` = 新流的第一次取帧。
     *
     * 这是数据面交接的账本：Kotlin 侧只在收到更大的 `after` 时才裁掉旧帧，故一次请求白跑
     * （回执迟到、future 被丢）不会丢帧。理由见 `StatsBridge.kt` 的类文档。
     */
    var after: Long = 0
}

/**
 * Rust ↔ Kotlin/libbox 起停核桥的 Kotlin 半边。
 *
 * 设计与判据：`~/docs/polaris/design/polaris-android-rust-kotlin-bridge-design-2026-09-04.md`。
 *
 * # 承重：`@Command` 里绝不阻塞
 *
 * `PluginManager.runCommand` 在**主线程**被调用 ⇒ 本类的每个 `@Command` 方法体都跑在 Android
 * 主线程上。起核是秒级（`startOrReloadService` 同步走完内核启动），在主线程等它 = ANR，且
 * `startForeground` 的 5 秒窗口也会被吃掉。故三个命令一律「立刻返回 + 把 `Invoke` 存起来」，
 * 由 [`VpnBridge`] 在 `BoxService` 的工作线程里结账。`Invoke` 是纯值对象（持两个 callback id +
 * 一个 `sendResponse` lambda），跨线程 resolve/reject 是它被设计出来的用法。
 *
 * # 错误码原样过桥
 *
 * `invoke.reject(msg, code)` 的 `code` 会被 Tauri 放进 `ErrorResponse.code`，Rust 侧据此落
 * `StartError::coded`。码的取值集由 Rust 侧白名单收口（见 `runtime/proxy/android_bridge.rs`
 * 的 `map_rejected_code`）—— 这里写的串不构成新码，只是在已有码里选一个。
 */
@TauriPlugin
class PolarisVpnPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun listBindableInterfaces(invoke: Invoke) {
        Thread {
            runCatching { InterfaceBoundSocket.interfaces() }
                .onSuccess { rows ->
                    invoke.resolve(JSObject().put("interfaces", JSONArray(rows.map(::bindableInterface))))
                }
                .onFailure { invoke.reject("Android network interface enumeration failed", "NETWORK_INTERFACE_LIST_FAILED") }
        }.start()
    }

    private fun bindableInterface(row: BindableInterface): JSObject = JSObject()
        .put("name", row.name)
        .put("displayName", row.displayName)
        .put("isUp", row.isUp)
        .put("addresses", JSONArray(row.addresses))

    @Command
    fun startTransientLogin(invoke: Invoke) {
        val args = invoke.parseArgs(TransientLoginStartArgs::class.java)
        TransientLoginHost.start(args.instanceId, args.configContent) { failure ->
            if (failure == null) invoke.resolve()
            else invoke.reject(
                failure.message,
                if (failure is TransientLoginHost.SystemInterfaceFailure) SystemEndpointGuard.ERROR else "TAILSCALE_LOGIN_FAILED",
            )
        }
    }

    @Command
    fun closeTransientLogin(invoke: Invoke) {
        val args = invoke.parseArgs(TransientLoginInstanceArgs::class.java)
        TransientLoginHost.close(args.instanceId) { failure ->
            if (failure == null) invoke.resolve() else invoke.reject(failure, "TAILSCALE_LOGIN_CANCEL_FAILED")
        }
    }

    @Command
    fun transientLoginStatus(invoke: Invoke) {
        val args = invoke.parseArgs(TransientLoginInstanceArgs::class.java)
        invoke.resolve(JSObject().put("running", TransientLoginHost.running(args.instanceId)))
    }

    @Command
    fun startTransientSpeedtest(invoke: Invoke) {
        val args = invoke.parseArgs(TransientSpeedtestStartArgs::class.java)
        TransientSpeedtestHost.start(args.instanceId, args.configContent) { failure ->
            if (failure == null) invoke.resolve()
            else invoke.reject(failure, "TRANSIENT_SPEEDTEST_FAILED")
        }
    }

    @Command
    fun closeTransientSpeedtest(invoke: Invoke) {
        val args = invoke.parseArgs(TransientSpeedtestInstanceArgs::class.java)
        TransientSpeedtestHost.close(args.instanceId) { failure ->
            if (failure == null) invoke.resolve()
            else invoke.reject(failure, "TRANSIENT_SPEEDTEST_CLEANUP_UNKNOWN")
        }
    }

    @Command
    fun transientSpeedtestStatus(invoke: Invoke) {
        val args = invoke.parseArgs(TransientSpeedtestInstanceArgs::class.java)
        invoke.resolve(JSObject().put("state", TransientSpeedtestHost.status(args.instanceId)))
    }

    @Command
    fun collectDebugDiagnostics(invoke: Invoke) {
        if (!BuildConfig.DEBUG) { invoke.reject("Debug reports are disabled"); return }
        Thread {
            runCatching { DebugDiagnostics.collect(activity.applicationContext) }
                .onSuccess { invoke.resolve(JSObject().put("text", it)) }
                .onFailure { invoke.reject("Could not collect Android diagnostics") }
        }.start()
    }

    @Command
    fun shareDebugReport(invoke: Invoke) {
        if (!BuildConfig.DEBUG) { invoke.reject("Debug reports are disabled"); return }
        val args = invoke.parseArgs(DebugReportArgs::class.java)
        if (args.report.toByteArray(Charsets.UTF_8).size > 4 * 1024 * 1024) {
            invoke.reject("Report is too large")
            return
        }
        Thread {
            runCatching {
                val dir = File(activity.cacheDir, "debug-reports")
                check(dir.isDirectory || dir.mkdirs())
                // Immutable per export: a receiving app may read the URI after another export.
                val file = File(dir, "polaris-debug-${System.currentTimeMillis()}-${UUID.randomUUID()}.md")
                file.writeText(args.report)
                dir.listFiles()?.filter { it.name.startsWith("polaris-debug-") && it.extension == "md" }
                    ?.sortedByDescending { it.lastModified() }?.drop(5)?.forEach { it.delete() }
                FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", file)
            }.onSuccess { uri ->
                activity.runOnUiThread {
                    runCatching {
                        check(!activity.isFinishing && !activity.isDestroyed)
                        val intent = Intent(Intent.ACTION_SEND).apply {
                            type = "text/plain"
                            putExtra(Intent.EXTRA_STREAM, uri)
                            clipData = ClipData.newRawUri("Polaris diagnostic report", uri)
                            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                        }
                        activity.startActivity(Intent.createChooser(intent, null))
                    }.onSuccess { invoke.resolve() }
                        .onFailure { invoke.reject("Could not open Android share sheet") }
                }
            }.onFailure { invoke.reject("Could not prepare diagnostic report") }
        }.start()
    }

    // 只在主线程读写。取消后仍保留窗口占位，直到旧回调归来，避免覆盖 Tauri 的单个结果回调。
    private var pendingVpnPermission: Invoke? = null
    private var vpnPermissionCancelled = false

    /** 授权独立于起核：用户阅读系统弹窗的时间不占用 Rust 的 30 秒起核预算。 */
    @Command
    fun requestVpnPermission(invoke: Invoke) {
        if (pendingVpnPermission != null) {
            invoke.reject("VPN 授权窗口已打开，请先完成当前授权", ERR_VPN_PERMISSION_DENIED)
            return
        }
        try {
            val intent = VpnService.prepare(activity)
            if (intent == null) {
                invoke.resolve()
                return
            }
            // 后台恢复不能主动打开系统窗口；用户回到前台连接时再请求。
            if (activity.isFinishing || activity.isDestroyed ||
                (activity as? LifecycleOwner)?.lifecycle?.currentState
                    ?.isAtLeast(Lifecycle.State.RESUMED) != true
            ) {
                invoke.reject("请回到 Polaris 点击连接以授予 VPN 权限", ERR_VPN_PERMISSION_DENIED)
                return
            }
            pendingVpnPermission = invoke
            vpnPermissionCancelled = false
            startActivityForResult(invoke, intent, "vpnPermissionResult")
        } catch (e: Exception) {
            pendingVpnPermission = null
            Log.w(TAG, "打开 VPN 授权窗口失败", e)
            invoke.reject("无法打开 VPN 授权窗口：${e.message}", ERR_VPN_PERMISSION_DENIED)
        }
    }

    @ActivityCallback
    fun vpnPermissionResult(invoke: Invoke, result: ActivityResult) {
        if (pendingVpnPermission !== invoke) return
        pendingVpnPermission = null
        if (vpnPermissionCancelled) return
        try {
            if (result.resultCode == Activity.RESULT_OK && VpnService.prepare(activity) == null) {
                // 只回报授权，起核仍由 Rust 在检查本次连接未被取消后驱动。
                invoke.resolve()
            } else {
                invoke.reject("VPN 授权已取消或未获允许，请再次点击连接重试", ERR_VPN_PERMISSION_DENIED)
            }
        } catch (e: Exception) {
            invoke.reject("无法确认 VPN 授权：${e.message}", ERR_VPN_PERMISSION_DENIED)
        }
    }

    private fun cancelVpnPermission() {
        val pending = pendingVpnPermission ?: return
        if (vpnPermissionCancelled) return
        vpnPermissionCancelled = true
        pending.reject("VPN 连接已取消", ERR_VPN_PERMISSION_DENIED)
    }

    override fun onDestroy(activity: AppCompatActivity) {
        cancelVpnPermission()
        pendingVpnPermission = null
        super.onDestroy(activity)
    }

    /**
     * 起核：把 config-engine 产出的**那一份字符串**交给 libbox。
     *
     * VPN 授权由 [requestVpnPermission] 完成；此处再次检查，防止等待期间授权被撤销。授权失败与「起核失败」
     * （去查节点/网络）完全不同 ⇒ 单独一条腿、单独一个码，不压成 `STARTUP_FAILED`。
     */
    @Command
    fun start(invoke: Invoke) {
        val args = invoke.parseArgs(MainStartArgs::class.java)
        val cfg = args.configContent
        if (args.runId.isBlank() || args.runId.length > 128 || args.runId != args.runId.trim() ||
            !Regex("[0-9a-f]{64}").matches(args.configDigest) ||
            SystemStart.sha256(cfg.toByteArray(Charsets.UTF_8)) != args.configDigest ||
            (args.claim != null && (args.claim!!.isBlank() || args.claim!!.length > 8192))) {
            invoke.reject("android: 起核身份或配置摘要无效", ERR_STARTUP_FAILED)
            return
        }
        if (VpnService.prepare(activity) != null) {
            invoke.reject("Android 未授予 VPN 权限", ERR_VPN_PERMISSION_DENIED)
            return
        }
        if (!VpnBridge.beginStart(MainStartRequest(cfg, args.runId, args.configDigest, args.claim), invoke)) {
            invoke.reject("Android 隧道已在运行或正在起停中", ERR_STARTUP_FAILED)
            return
        }
        try {
            ContextCompat.startForegroundService(
                activity,
                Intent(activity, PolarisVpnService::class.java),
            )
        } catch (e: Throwable) {
            // startForegroundService 自己就可能抛（后台启动限制 / 前台服务类型不被允许）。
            // 不接这一条 ⇒ 服务压根没起、也没人 resolve，Rust 侧只能等到超时 —— 那正是本批
            // 明令禁止的「静默没起来」。
            Log.e(TAG, "启动前台服务失败", e)
            VpnBridge.finishStart(e.message ?: e.toString())
        }
    }

    /**
     * 停核：要确定回执（与 Rust 侧 `kill_core_via_helper` 同构，停不掉就让调用方知道）。
     *
     * Rust 请求停核 = 应用的意图是「隧道应当断开」（用户点断开 / 重启前先停 / 退出收尾）⇒ 撤销
     * 系统发起起核的准入（[SystemStart.forget]），且**放在幂等早退之前**：本就没在跑时也要撤，
     * 否则一次「核已自己死掉 → 用户点断开」之后，系统下次拉起服务仍会把它连回去。
     * 重启腿随后的 `start` 成功会重新记下（[SystemStart.remember]），不丢自启能力。
     */
    @Command
    fun stop(invoke: Invoke) {
        cancelVpnPermission()
        SystemStart.forget(activity, "应用请求停核")
        when (VpnBridge.beginStop(invoke)) {
            VpnBridge.StopAdmission.AlreadyStopped -> {
                // 本就没在跑 ⇒ 幂等成功。停核腿必须是幂等的。
                invoke.resolve()
                return
            }
            VpnBridge.StopAdmission.Busy -> {
                invoke.reject("android: 主核关闭仍在进行，结果未知", "ANDROID_CORE_STOP_FAILED")
                return
            }
            VpnBridge.StopAdmission.Started -> Unit
        }
        BoxService.requestStop(activity)
    }

    /**
     * 起核前的内核闸门（`config_gate`）在 Android 上的取证腿。
     *
     * `Libbox.checkConfig` 只做 decode + initialize（`box.New`，不 `Start`）⇒ 不绑端口、不开
     * `cache.db`，可以安全地插在真核起来之前。但它仍不能放主线程：生产规模配置带二十余个
     * `.srs`，解析是毫秒到几十毫秒级。
     *
     * 返回约定：`{}`（无 `error` 键）= 内核收下；`{"error": "<e.message>"}` = 拒收。
     * **异常消息原样回传，绝不翻译或加前缀** —— Rust 侧 `parse_kernel_rejection` 的
     * `decode config: ` marker 就锚在这串的开头，加一个字都会把归因锚点破坏掉。
     */
    @Command
    fun checkConfig(invoke: Invoke) {
        val cfg = invoke.parseArgs(CheckArgs::class.java).configContent
        Thread({
            val err = runCatching {
                PolarisApplication.ensureSetup()
                Libbox.checkConfig(cfg)
            }.exceptionOrNull()
            val result = JSObject()
            if (err != null) {
                result.put("error", err.message ?: err.toString())
            }
            invoke.resolve(result)
        }, "polaris-check-config").start()
    }

    /**
     * VPN 授权状态的**只读**取证腿。
     *
     * 事实源与 [start] 里那次判断**逐字相同** —— `VpnService.prepare(activity)`：返回 `null` = 系统
     * 已授权本应用（起核不会再弹窗），非 `null` = 那个 Intent 就是待用户确认的系统弹窗。故这里不是
     * 第二份真值，而是同一个系统事实的读取口；两处若哪天不一致，起核那条腿会当场自曝
     * （它拿的是同一个调用的返回值）。
     *
     * **绝不在这里 `startActivityForResult`**：读状态的命令不许有副作用。授权弹窗只能由用户真的按下
     * 「连接」时经 [requestVpnPermission] 引出，读一次设置页就弹一次系统弹窗是骚扰。
     *
     * 放工作线程、而不是像 [start] 那样内联：`prepare` 是一次到 `system_server` 的 binder 往返。
     * [start] 那次是**一次性用户动作**的闸门（下一步就要把 `Invoke` 交出去），而本命令按**节奏**被调
     * （渲染端每次回到前台读一次），主线程上按节奏做 binder 往返正是本类头注那条承重约束要挡的形态。
     *
     * 回包：`{"authorized": true|false}`。键名与 Rust `AuthStatusResponse` 的 serde 字段逐字对齐，
     * 由 `scripts/check-android-bridge.mjs` 的 A10 双向对拍（Kotlin 多发/少发都红）。
     * 失败一律 `reject` 且**不带码**：读不到状态不是「未授权」，Rust 侧会折成 `unknown`，
     * 界面照实说「读不到」而不是编一个「未授权」。
     */
    @Command
    fun vpnAuthStatus(invoke: Invoke) {
        Thread({
            try {
                val result = JSObject()
                result.put("authorized", VpnService.prepare(activity) == null)
                invoke.resolve(result)
            } catch (e: Throwable) {
                Log.w(TAG, "读取 VPN 授权状态失败", e)
                invoke.reject(e.message ?: e.toString())
            }
        }, "polaris-vpn-auth").start()
    }

    // ── 不经桥起核（always-on / 开机 / 系统重拉）的对账与开机自启开关 ─────────────────────

    /**
     * 此刻是否有一个**不是本桥起的**内核在跑（系统经 always-on / 开机接收器拉起的）。
     *
     * 消费方是 Rust 的两处既有路径：启动期自动连接腿（有这种核 ⇒ 走一次标准起核把它收编，界面
     * 才不会停在「未连接」）与起核前的孤儿清扫（先把它停掉，再由 Rust 按当前配置重起）。
     * 只读桥内存里的两位，无 binder、无 I/O ⇒ 就地回包，不开线程。
     *
     * 回包：`{"systemStarted": true|false}`，键名与 Rust `SystemStartStatus` 由 A10 双向对拍。
     */
    @Command
    fun systemStartStatus(invoke: Invoke) {
        val result = JSObject()
        result.put("systemStarted", VpnBridge.systemStartedRunning())
        invoke.resolve(result)
    }

    /** Main-core ownership only. Caller must also prove temporary owners absent under its gate. */
    @Command
    fun mainCoreOwnership(invoke: Invoke) {
        val (state, runId) = MainKernelAttemptRegistry.snapshot()
        val result = JSObject().put("state", state)
        if (runId != null) result.put("runId", runId)
        invoke.resolve(result)
    }

    /**
     * 写「开机自动连接」开关（Rust `auto_start_set` 的 Android 腿；真值住 Kotlin，因为开机那一刻
     * 只有 [BootReceiver] 在跑）。放工作线程：落盘是 I/O。失败 `reject` 不带码，Rust 侧原话回显到
     * 设置页那一行下面。
     */
    @Command
    fun setBootAutoConnect(invoke: Invoke) {
        val enabled = invoke.parseArgs(BootAutoConnectArgs::class.java).enabled
        Thread({
            try {
                SystemStart.setBootAutoConnect(activity, enabled)
                invoke.resolve()
            } catch (e: Throwable) {
                Log.w(TAG, "写开机自动连接开关失败", e)
                invoke.reject(e.message ?: e.toString())
            }
        }, "polaris-boot-flag").start()
    }

    /** 读「开机自动连接」开关。回包 `{"enabled": true|false}`，键名与 Rust `BootAutoConnectStatus` 由 A10 对拍。 */
    @Command
    fun bootAutoConnectStatus(invoke: Invoke) {
        Thread({
            try {
                val result = JSObject()
                result.put("enabled", SystemStart.bootAutoConnect(activity))
                invoke.resolve(result)
            } catch (e: Throwable) {
                Log.w(TAG, "读开机自动连接开关失败", e)
                invoke.reject(e.message ?: e.toString())
            }
        }, "polaris-boot-flag").start()
    }

    // ── 系统自动备份开关（设置 → 通用「系统备份」）────────────────────────────────

    /**
     * 写「系统备份」开关（Rust `system_backup_set`）。真值住 Kotlin 的 `noBackupFilesDir`：系统做备份时
     * 只有 [PolarisBackupAgent] 在跑（受限模式、Rust 不在），它读的就是这一份。放工作线程：落盘是 I/O。
     */
    @Command
    fun setSystemBackup(invoke: Invoke) {
        val enabled = invoke.parseArgs(SystemBackupArgs::class.java).enabled
        Thread({
            try {
                PolarisBackupAgent.setEnabled(activity, enabled)
                invoke.resolve()
            } catch (e: Throwable) {
                Log.w(TAG, "写系统备份开关失败", e)
                invoke.reject(e.message ?: e.toString())
            }
        }, "polaris-backup-flag").start()
    }

    /** 读「系统备份」开关。回包 `{"enabled": true|false}`，键名与 Rust `SystemBackupStatus` 由 A10 对拍。 */
    @Command
    fun systemBackupStatus(invoke: Invoke) {
        Thread({
            try {
                val result = JSObject()
                result.put("enabled", PolarisBackupAgent.isEnabled(activity))
                invoke.resolve(result)
            } catch (e: Throwable) {
                Log.w(TAG, "读系统备份开关失败", e)
                invoke.reject(e.message ?: e.toString())
            }
        }, "polaris-backup-flag").start()
    }

    // ── W-09b：已装应用枚举（自定义应用分流的包名来源）──────────────────────────
    //
    // 为什么非有这条腿不可：`CustomAppPreset` 只带 `processNames`（桌面语义的进程名，
    // 用户填的是 "Netflix" / "chrome.exe"），而 Android 侧 `addDisallowedApplication` 认的是
    // applicationId。`crates/config-engine/src/user_config/app_rules_preset.rs` 里自定义预设的
    // `package_names` 因此恒空 —— 「自定义应用设成直连」在 Android 上一条规则都命不中，
    // 而且是**静默**的（那个 API 对不认识的包名抛 NameNotFoundException，catch 掉就没了）。
    // 补的正是「包名从哪儿来」这一格。

    /**
     * 枚举**用户可见的**已装应用（= 有启动器图标的那些），供「自定义应用」的表单挑包名。
     *
     * # 取材面：带 LAUNCHER 的 activity，不是全部已装包
     *
     * 判据是 `Intent(ACTION_MAIN) + CATEGORY_LAUNCHER` 的 `queryIntentActivities` —— 它返回的
     * 恰好是「用户能在桌面点开的应用」，也就是用户在分流界面上认得出来的那批。无界面的服务、
     * 输入法、系统组件不在其中，那正是本命令**不要** `QUERY_ALL_PACKAGES` 的理由
     * （路线声明见 [PACKAGE_VISIBILITY_ROUTE]）。
     *
     * # 不带图标（如实登记的射程收窄）
     *
     * 回包只有 `label` / `packageName` / `system` 三列，**没有图标**。一台普通手机有 100+ 个带
     * 启动器图标的应用，每个图标转 base64 进 JSON 是几十 KB 起步 —— 这条命令会从「一次几十 KB
     * 的往返」变成「几 MB 的往返」，而它跑在 Tauri 的 IPC 上。要图标的正解是另开一条
     * 「按包名取一个图标」的命令、由列表按需拉取，不是把整包塞进这一条。
     *
     * 回包：`{"apps": [{label, packageName, system}, …]}`。键名与 Rust `InstalledAppsResponse` /
     * `InstalledApp` 的 serde 字段逐字对齐，由 `scripts/check-android-bridge.mjs` 的 A10（顶层）
     * 与 A12（元素）双向对拍 —— 任一侧改名，门当场红。
     *
     * 放工作线程：`queryIntentActivities` 是一次到 `package_manager` 的 binder 往返，且要为每个
     * 结果 `loadLabel`（读对方 APK 的资源）。主线程上做这件事正是本类头注那条承重约束要挡的形态。
     */
    @Command
    fun listInstalledApps(invoke: Invoke) {
        Thread({
            try {
                val apps = JSArray()
                for (entry in launcherApps()) apps.put(entry)
                val result = JSObject()
                result.put("apps", apps)
                invoke.resolve(result)
            } catch (e: Throwable) {
                // 读不到就说读不到。**绝不 resolve 一个空列表** —— 那与「这台机器上真的没有
                // 第三方应用」不可区分，而后者几乎不可能，前者（包可见性没声明对）很可能。
                Log.w(TAG, "枚举已装应用失败", e)
                invoke.reject(e.message ?: e.toString())
            }
        }, "polaris-installed-apps").start()
    }

    // ── W-21：把下载好的 APK 交给系统安装器 ──────────────────────────────────────
    //
    // 分工：**下载归 Rust**（`update_download` 已经带三级完整性判据 + 单飞闸 + 进度事件，落到
    // `app_cache_dir()/updates/`，那就是应用私有目录），**交付归 Kotlin**（FileProvider 授一个
    // 一次性 read 权限 + ACTION_VIEW）。Kotlin 侧再写一份下载就是第二份真值。

    /**
     * 把已下载的 APK 交给系统安装器。
     *
     * # 三条会「什么都不发生」的路，逐条给码而不是吞掉
     *
     * | 情形 | 回包 `reason` | 用户的下一步 |
     * |---|---|---|
     * | 没授予「安装未知应用」（API 26+ 按应用授权） | [REASON_UNKNOWN_SOURCES_DENIED] | 本命令**已经把他送到那一页**，授权后重来 |
     * | 同上，且设置页也打不开（定制 ROM 摘了那个 Activity） | [REASON_UNKNOWN_SOURCES_NO_SETTINGS] | 只能自己去系统设置里找 |
     * | 本机没有能处理安装 intent 的组件 | [REASON_NO_INSTALLER] | 本机装不了，去别处装 |
     * | 路径不在应用私有 cache 目录里 | [REASON_NOT_APP_PRIVATE] | 这是接线错误，不是用户能修的 |
     * | 路径上没有这个文件 | [REASON_PACKAGE_MISSING] | 先下载 |
     *
     * 🔴 **不许静默失败**是本条腿的承重约束：`startActivity` 在没授权时不会抛异常，系统只是
     * 不装（或弹一个不说原因的框），于是「点了更新什么都没发生」。故授权判据必须在
     * `startActivity` **之前**用 `canRequestPackageInstalls()` 显式问一次，并把答案带回 Rust。
     * 判据在 `src-tauri/tests/android_native_surface_wiring.rs`（安装器不静默失败门）。
     *
     * # 为什么这里可以有副作用（对比 [vpnAuthStatus] 的「读状态不许弹窗」）
     *
     * [vpnAuthStatus] 是按**节奏**被调的只读腿，它弹任何东西都是骚扰。本命令相反：它由用户
     * 亲手按下「安装」引出，一次用户动作对一次跳转。把用户送到「安装未知应用」那一页
     * 正是那次动作的**延续**，不是意外 —— 反过来，只回一个错误码而不给路，才是把人晾在原地。
     *
     * # 为什么不用 `PackageInstaller` 会话式
     *
     * 会话式要自己起一个 `IntentSender` 接收器、自己处理 `STATUS_PENDING_USER_ACTION`，
     * 换来的能力（后台流式写入、静默更新）本仓一样都用不上：包已经在盘上，且我们没有
     * device owner 权限，最终照样要弹系统确认框。ACTION_VIEW 少一整套状态机，
     * 失败面也小 —— 这是刻意选的窄路，不是不知道有另一条。
     *
     * 回包：`{"handedOff": bool, "reason": string?}`（成功时无 `reason` 键 ——
     * `JSONObject.put(key, null)` 会删键，Rust 侧 `#[serde(default)]` 收）。
     */
    @Command
    fun installApk(invoke: Invoke) {
        val apkPath = invoke.parseArgs(InstallApkArgs::class.java).apkPath
        Thread({
            try {
                val outcome = handOffToSystemInstaller(apkPath)
                val result = JSObject()
                result.put("handedOff", outcome.handedOff)
                result.put("reason", outcome.reason)
                invoke.resolve(result)
            } catch (e: Throwable) {
                // 走到这里说明连「为什么交不出去」都没算出来（FileProvider 授权面没配、
                // canonicalFile 触到 I/O 错误…）。原样回传，Rust 侧会落成一个有原因的失败 ——
                // 吞掉它就又回到「点了更新什么都没发生」。
                Log.e(TAG, "交系统安装器失败", e)
                invoke.reject(e.message ?: e.toString())
            }
        }, "polaris-install-apk").start()
    }

    // ── 上面两条命令的实现腿（**刻意放在 `@Command` 方法体之外**）────────────────
    //
    // `scripts/check-android-bridge.mjs` 的 A10 拿的是 `@Command` 方法体里**全部**深度的
    // `put("键")`，与 Rust 回包类型的 serde 字段面逐条对拍。把逐条目的 `put` 写进命令体，
    // A10 就会拿元素的键去比顶层结构体的字段面 —— 判据打在错的对象上。
    // 分出来之后：顶层由 A10 守（`apps` / `handedOff` / `reason`），元素由 A12 守
    // （`installedApp` ⇄ Rust `InstalledApp`，函数名与结构体名同源，不维护映射表）。

    /** [handOffToSystemInstaller] 的结局：交出去了没有 + 没交出去的**原因码**。 */
    private class Handoff(val handedOff: Boolean, val reason: String?)

    /** 已装应用列表（按显示名排序、按包名去重、剔除本应用自己）。 */
    private fun launcherApps(): List<JSObject> {
        val pm = activity.packageManager
        val intent = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        val resolved = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            pm.queryIntentActivities(intent, PackageManager.ResolveInfoFlags.of(0L))
        } else {
            // minSdk = 24 ⇒ 这一支是真的会跑的，不是形式主义。
            @Suppress("DEPRECATION")
            pm.queryIntentActivities(intent, 0)
        }
        val seen = HashSet<String>()
        val out = ArrayList<JSObject>()
        for (info in resolved) {
            val pkg = info.activityInfo?.packageName ?: continue
            // 本应用不进候选：「把自己排除出隧道」是 VpnService 那侧的事
            // （`addDisallowedApplication(自己)`），不该让用户在分流列表里再选一次。
            if (pkg == activity.packageName) continue
            // 一个包可以有多个带 LAUNCHER 的 activity（多入口应用），按包名去重。
            if (!seen.add(pkg)) continue
            out.add(installedApp(pm, info))
        }
        out.sortWith(
            compareBy(
                { it.optString("label").lowercase(Locale.ROOT) },
                { it.optString("packageName") },
            ),
        )
        if (out.isEmpty()) {
            // 空列表在这条腿上几乎必然是**声明问题**而不是事实：targetSdk ≥ 30 时包可见性过滤
            // 生效，`<queries>` 没声明对就恰好长这样（不报错、不抛异常）。留一行能归因的日志。
            Log.w(
                TAG,
                "已装应用枚举为空（包可见性路线=$PACKAGE_VISIBILITY_ROUTE）—— targetSdk≥30 上" +
                    "这通常是 AndroidManifest 的 <queries> 没生效，而不是机器上真的没有应用",
            )
        }
        return out
    }

    /**
     * 一条已装应用的回包元素。
     *
     * 键名 ⇄ Rust `InstalledApp` 的 serde 字段面由 A12 双向对拍；函数名 `installedApp` 与结构体名
     * `InstalledApp` 是**同一个名字**，这条配对因此不需要任何映射表 —— 任一侧改名，A12 的 FLOOR
     * 会因为配对数掉到 0 而转红。
     *
     * `system` 是给渲染端分组用的（系统应用与第三方应用混在一张表里很难挑）；后端不消费它。
     */
    private fun installedApp(pm: PackageManager, info: ResolveInfo): JSObject {
        val entry = JSObject()
        entry.put("label", info.loadLabel(pm).toString())
        entry.put("packageName", info.activityInfo.packageName)
        entry.put(
            "system",
            (info.activityInfo.applicationInfo.flags and ApplicationInfo.FLAG_SYSTEM) != 0,
        )
        return entry
    }

    /** [installApk] 的实现腿（结局表见该命令的文档）。 */
    private fun handOffToSystemInstaller(apkPath: String): Handoff {
        val apk = File(apkPath).canonicalFile
        val cacheRoot = activity.cacheDir.canonicalFile
        // 🔴 只认应用私有 cache 目录下的包。**这一条前缀判据是唯一的落点约束** ——
        // 别指望 FileProvider 的配置替它把关：`res/xml/file_paths.xml` 的授权面**宽于** APK 落点，
        // 除了 `<cache-path>` 还有一条 `<external-path name="my_images" path="." />`（Tauri 模板
        // 带来的），那把整个共享外部存储根也声明进了同一个 provider。所以对 `/sdcard/x.apk`
        // 这种路径，`getUriForFile` 会**正常返回**一个 content:// URI，不会抛
        // IllegalArgumentException —— 把「FileProvider 已经拦住了」当成理由，等于把这条判据的
        // 必要性说没了，而删掉它之后非私有落点会被真的授出去。
        //
        // 为什么需要这条：本命令的入参最终来自一次 IPC。接受任意路径 = 把「给系统安装器授哪个
        // 文件的读权限」交给调用方决定。`canonicalFile` 先做符号链接归一化（两端都做），
        // `..` 逃逸在这一步就死了。
        // 判据：`src-tauri/tests/android_native_surface_wiring.rs` 的
        // `installer_refuses_packages_outside_the_private_cache`（含极性对照）。
        if (!apk.path.startsWith(cacheRoot.path + File.separator)) {
            Log.e(TAG, "拒绝交付：APK 不在应用私有目录里（${apk.path}）")
            return Handoff(false, REASON_NOT_APP_PRIVATE)
        }
        if (!apk.isFile) {
            Log.e(TAG, "拒绝交付：APK 不存在（${apk.path}）")
            return Handoff(false, REASON_PACKAGE_MISSING)
        }

        // 🔴 授权判据必须在 startActivity **之前**：没授权时 startActivity 不抛异常，
        // 系统只是不装 —— 那就是「点了更新什么都没发生」，本批明令禁止的形态。
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
            !activity.packageManager.canRequestPackageInstalls()
        ) {
            return Handoff(false, openUnknownSourcesSettings())
        }

        val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", apk)
        val intent = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, APK_MIME)
            // content:// 的一次性读授权。少了它，安装器拿到 uri 也读不动。
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            // 本方法跑在工作线程、且从非 Activity 上下文语义起跳，必须显式给新任务栈。
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return try {
            activity.startActivity(intent)
            Handoff(true, null)
        } catch (e: ActivityNotFoundException) {
            Log.e(TAG, "本机没有能处理 APK 安装 intent 的组件", e)
            Handoff(false, REASON_NO_INSTALLER)
        }
    }

    /**
     * 把用户送到「安装未知应用」那一页，并回报本次为什么没装成。
     *
     * 两个码不许合并：[REASON_UNKNOWN_SOURCES_DENIED] 是「按一下开关就能解决，而且我已经把你
     * 送到那个开关面前了」，[REASON_UNKNOWN_SOURCES_NO_SETTINGS] 是「这条路走不通，你得自己去
     * 系统设置里找」。折成一个码等于对后一种情形的用户说一句做不到的话。
     */
    private fun openUnknownSourcesSettings(): String {
        // API < 26 上没有按应用的「安装未知应用」这个对象（那时是一个全局开关），
        // 调用点也不会走到这里（那一支被 SDK_INT 判据挡在外面）。留这一行是为了让本函数
        // 自己是全的：它不依赖调用点替它守版本。
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return REASON_UNKNOWN_SOURCES_DENIED
        val intent = Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES)
            .setData(Uri.parse("package:${activity.packageName}"))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return try {
            activity.startActivity(intent)
            REASON_UNKNOWN_SOURCES_DENIED
        } catch (e: ActivityNotFoundException) {
            Log.e(TAG, "打不开「安装未知应用」设置页", e)
            REASON_UNKNOWN_SOURCES_NO_SETTINGS
        }
    }

    // ── 数据面：连接列表 + 流量统计 ──────────────────────────────────────────────
    //
    // 三命令一组（open / poll / close），每组对应一条 libbox 命令通道，与桌面的
    // 「建流 → recv → drop 流」逐条同构（`runtime/stats/relay.rs`）。实现与判据见
    // `StatsBridge.kt` 与 `~/docs/polaris/design/polaris-android-dataplane-2026-09-04.md`。
    //
    // 六个方法体一律「立刻返回 + 由 StatsBridge 的 worker 线程结账」，理由同本类的承重约束：
    // `@Command` 跑在主线程，而 poll 是**长轮询**（最多停 900ms 等帧）。

    /** 建 stats 流（libbox `CommandStatus`）。 */
    @Command
    fun statsOpen(invoke: Invoke) {
        StatsBridge.statsStream.open { err ->
            if (err == null) invoke.resolve() else invoke.reject(err)
        }
    }

    /** 取 stats 帧。 */
    @Command
    fun statsPoll(invoke: Invoke) {
        val after = invoke.parseArgs(PollArgs::class.java).after
        StatsBridge.statsStream.poll(after) { invoke.resolve(it) }
    }

    /** 收 stats 流（对应桌面「门关 ⇒ drop 流」）。 */
    @Command
    fun statsClose(invoke: Invoke) {
        StatsBridge.statsStream.close()
        invoke.resolve()
    }

    /** 建连接流（libbox `CommandConnections`）。 */
    @Command
    fun connectionsOpen(invoke: Invoke) {
        StatsBridge.connectionsStream.open { err ->
            if (err == null) invoke.resolve() else invoke.reject(err)
        }
    }

    /** 取连接事件帧。 */
    @Command
    fun connectionsPoll(invoke: Invoke) {
        val after = invoke.parseArgs(PollArgs::class.java).after
        StatsBridge.connectionsStream.poll(after) { invoke.resolve(it) }
    }

    /** 收连接流。 */
    @Command
    fun connectionsClose(invoke: Invoke) {
        StatsBridge.connectionsStream.close()
        invoke.resolve()
    }

    companion object {
        private const val TAG = "PolarisVpnPlugin"

        /** 与 Rust `runtime/proxy::code::VPN_PERMISSION_DENIED` 逐字对齐。 */
        const val ERR_VPN_PERMISSION_DENIED = "VPN_PERMISSION_DENIED"

        /** 与 Rust `runtime/proxy::code::STARTUP_FAILED` 逐字对齐。 */
        const val ERR_STARTUP_FAILED = "STARTUP_FAILED"

        /**
         * 包可见性路线（Android 11 / API 30 起的 package visibility filtering）——
         * **本仓走 `<queries>`，不要 `QUERY_ALL_PACKAGES`**。
         *
         * # 事实前提（不是选型偏好，是 API 约束）
         *
         * `app/build.gradle.kts` 里 `targetSdk = 36`（≥ 30）⇒ 包可见性过滤**生效**：
         * `queryIntentActivities` / `getInstalledPackages` 只返回「对本应用可见」的那部分包，
         * 而默认可见集**不含**普通第三方应用。什么都不声明的后果不是编译错、不是运行时异常，
         * 是真机上拿到一个**空列表** —— 而在低 targetSdk 的旧机器/某些模拟器上它反而是满的，
         * 这正是这条坑容易被「本地看着好好的」放过去的原因。
         *
         * # 两条路，为什么选窄的
         *
         * | 路线 | 拿到什么 | 代价 |
         * |---|---|---|
         * | `<queries>` + 带 LAUNCHER 的 intent | 用户能在桌面点开的应用 —— 恰好是分流要选的集合 | 无 |
         * | `QUERY_ALL_PACKAGES` 权限 | 全部已装包（含无界面服务 / 输入法 / 系统组件） | Play 视为敏感权限需单独说明；把权限画像整体放宽 |
         *
         * 本仓走 GitHub Releases 分发、不上架，Play 的审核成本对我们不存在 —— **但权限画像仍按
         * 最小必要**：分流列表要的就是「有启动器图标的应用」，`QUERY_ALL_PACKAGES` 多拿到的那部分
         * 在这个界面上一条都用不上。多要一个敏感权限换零收益是纯负债。
         *
         * # 这个常量不是注释，是**判据物**
         *
         * `src-tauri/tests/android_native_surface_wiring.rs` 的包可见性门读它，并要求它与
         * `AndroidManifest.xml` 的事实一致：取 `queries-launcher` ⇒ manifest 必须有带
         * MAIN + LAUNCHER 的 `<queries>` 且**不得**声明 `QUERY_ALL_PACKAGES`；取
         * `query-all-packages` ⇒ 反过来。写一句注释说「我们走 queries」与 manifest 里真的写了
         * 什么，是两份可以各自漂的东西；写成常量之后它们被一道门钉在一起。
         */
        const val PACKAGE_VISIBILITY_ROUTE = "queries-launcher"

        /** APK 的 MIME —— 系统安装器就是靠它认领 `ACTION_VIEW` 的。 */
        const val APK_MIME = "application/vnd.android.package-archive"

        /** 没授予「安装未知应用」；本次已把用户送到那一页，授权后重来即可。 */
        const val REASON_UNKNOWN_SOURCES_DENIED = "unknown-sources-denied"

        /** 没授予，且连那一页都打不开（定制 ROM 摘掉了那个 Activity）。 */
        const val REASON_UNKNOWN_SOURCES_NO_SETTINGS = "unknown-sources-settings-unavailable"

        /** 给的路径不在应用私有 cache 目录里 —— 接线错误，不是用户能修的。 */
        const val REASON_NOT_APP_PRIVATE = "package-not-app-private"

        /** 路径上没有这个文件（多半是还没下载 / 下载被清理了）。 */
        const val REASON_PACKAGE_MISSING = "package-missing"

        /** 本机没有能处理 APK 安装 intent 的组件。 */
        const val REASON_NO_INSTALLER = "no-installer-activity"
    }
}

/**
 * 桥的进程级交接点：`@Command` 存 `Invoke` + 本次配置，`BoxService` 的工作线程取出并结账。
 *
 * # 为什么是进程级单例而不是把 `Invoke` 传给 Service
 *
 * `startForegroundService` 只能递 `Intent`（Parcelable 载荷），而 `Invoke` 持的是两个回调 id 与
 * 一个闭包，跨 `Intent` 传不了；生产规模配置几百 KB 也不该塞进 Binder 事务（`TransactionTooLarge`）。
 * 进程级单例是唯一的接法，与 `PolarisApplication` 同构（Go 侧回调也是靠它拿 Context）。
 *
 * # 状态机
 *
 * `Stopped → Starting → Started → Stopping → Stopped`，与 [`ServiceState`] 同名同义但**不是**
 * 第二份真值：这里存的是「桥欠着谁一个回执」，`BoxService.state` 存的是「服务自己走到哪一步」。
 * 桥必须自己记一份，否则「起核请求还没送到服务」这一小段窗口里 `BoxService` 还是 `Stopped`，
 * 第二次 `start` 会被放行、于是两个 `Invoke` 抢同一条链路。
 */
internal object VpnBridge {
    private const val TAG = "PolarisVpnBridge"

    private var pendingStart: Invoke? = null
    private var pendingStop: Invoke? = null
    private var starting = false
    private var stopping = false

    /**
     * 本次起核要交给内核的配置。
     *
     * **唯一来源是 Rust 侧 config-engine 的产出**（经 `start` 命令的 `configContent` 参数进来），
     * 与落盘那一份是**同一个字符串**，不是两次序列化 ⇒ 诊断包里的那份与内核实际吃的那份
     * 不可能漂。Kotlin 侧不解析、不改写、不兜底 —— 一旦这里出现任何硬编码配置，就又有了第二份
     * 真值（`scripts/check-android-bridge.mjs` 的 A4 断言正是守这件事）。
     *
     * `null` 且服务被拉起 ⇒ 那是**不经桥**的起核（系统发起），配置改由 [SystemStart.load] 从 Rust
     * 落的**同一个文件**读 —— 仍是同一份字节，不是 Kotlin 自存的第二份（见 [SystemStart] 类文档）。
     */
    @Volatile
    private var request: MainStartRequest? = null

    /** 起核是否已经成功过一次（`is_alive` 的桥侧真值；见 Rust `android_bridge::core_started`）。 */
    @Volatile
    var running: Boolean = false
        private set

    /**
     * 当前（在起或在跑的）这个核是**不经桥**起的（系统拉起服务、配置来自 [SystemStart.load]）。
     * Rust 据此判「有一个我不认识的核在跑」—— 见 `systemStartStatus` 命令。
     */
    private var systemStarted = false

    @Synchronized
    fun beginStart(startRequest: MainStartRequest, invoke: Invoke): Boolean {
        if (running || starting || stopping) return false
        starting = true
        systemStarted = false
        request = startRequest
        pendingStart = invoke
        return true
    }

    /**
     * 服务在**没有桥调用**的情况下被拉起（always-on / 开机接收器 / 系统重拉）。只做记账：让桥知道
     * 此刻有一个起核在飞（挡住并发的 `start`），且这个核不是本桥起的。`false` = 桥此刻不空闲。
     */
    @Synchronized
    fun beginSystemStart(): Boolean {
        if (running || starting || stopping) return false
        starting = true
        systemStarted = true
        return true
    }

    @Synchronized
    fun systemStartedRunning(): Boolean = running && systemStarted

    /**
     * 服务不在 `Stopped`、这一次起核意图走不下去时的结账：**只**结掉待决的桥回执（若有）。
     *
     * 与 [finishStart] 分开的理由：那个方法会把 `running` 改成 `false`。系统在隧道已经在跑时再发一次
     * 起服务意图（always-on 重申）是常态，没有任何人欠回执 —— 那时把 `running` 清掉，之后 Rust 的
     * `stop` 会被判成「本就没在跑」而幂等早退，隧道却还在。
     */
    @Synchronized
    fun rejectPendingStart(error: String) {
        val invoke = pendingStart
        if (invoke == null) {
            Log.i(TAG, "服务已在运行/起停中，忽略一次无人等待的起核意图（$error）")
            return
        }
        pendingStart = null
        starting = false
        invoke.reject(error, PolarisVpnPlugin.ERR_STARTUP_FAILED)
    }

    /** `BoxService` 起核线程读走本次配置。 */
    @Synchronized
    fun currentConfig(): String? = request?.configContent

    @Synchronized
    fun currentStartRequest(): MainStartRequest? = request

    @Synchronized
    fun isStopping(): Boolean = stopping

    /** 结账起核：`error == null` 即成功。幂等（重复调用只记日志）。 */
    @Synchronized
    fun finishStart(
        error: String?,
        code: String = PolarisVpnPlugin.ERR_STARTUP_FAILED,
        attempt: MainKernelAttempt<*>? = null,
    ): Boolean {
        val invoke = pendingStart
        val startRequest = request
        val receiptError = if (error == null && invoke != null &&
            (attempt == null || startRequest == null || attempt.runId != startRequest.runId)) {
            "android: 起核回执与当前主核身份不一致"
        } else null
        val failure = error ?: receiptError
        pendingStart = null
        starting = false
        running = failure == null
        if (failure != null) systemStarted = false
        if (invoke == null) {
            // 系统发起的起核本就没有人欠回执（error=null 时属正常）；桥发起的起核走到这里才是异常。
            Log.i(TAG, "起核结账时没有待决的 Invoke（systemStarted=$systemStarted, error=$failure）")
            return failure == null
        }
        if (failure == null) {
            val delivered = runCatching {
                check(attempt != null && startRequest != null)
                val response = JSObject()
                    .put("runId", attempt.runId)
                    .put("configDigest", startRequest.configDigest)
                startRequest.claim?.let { response.put("claim", it) }
                attempt.currentTunScope()?.let { scope ->
                    response.put("tun", JSObject()
                        .put("autoRoute", scope.autoRoute)
                        .put("routes", JSONArray(scope.routes))
                        .put("excludedRoutes", JSONArray(scope.excludedRoutes))
                        .put("skippedExcludes", JSONArray(scope.skippedExcludes))
                        .put("allowedPackages", JSONArray(scope.allowedPackages))
                        .put("excludedPackages", JSONArray(scope.excludedPackages))
                        .put("skippedPackages", JSONArray(scope.skippedPackages)))
                }
                invoke.resolve(response)
            }
            if (delivered.isFailure) {
                running = false
                Log.e(TAG, "主核回执投递失败，关闭该 attempt", delivered.exceptionOrNull())
            }
            return delivered.isSuccess
        } else {
            runCatching { invoke.reject(failure, code) }
                .onFailure { Log.e(TAG, "主核失败回执投递失败，继续关闭该 attempt", it) }
            return false
        }
    }

    enum class StopAdmission { Started, AlreadyStopped, Busy }

    /** A second request must not replace the first pending Stop acknowledgement. */
    @Synchronized
    fun beginStop(invoke: Invoke): StopAdmission {
        if (stopping) return StopAdmission.Busy
        if (!running && !starting) return StopAdmission.AlreadyStopped
        stopping = true
        pendingStop = invoke
        return StopAdmission.Started
    }

    /** 结账停核。服务无论因何停下（用户撤销授权 / 系统回收 / 我方请求）都必须走到这里。 */
    @Synchronized
    fun finishStop(error: String? = null) {
        val invoke = pendingStop
        pendingStop = null
        stopping = false
        if (error != null) {
            // The core is not usable, but its ownership remains reserved until a later confirmed close.
            running = false
            stopping = true
            val startInvoke = pendingStart
            pendingStart = null
            starting = false
            startInvoke?.reject(error, PolarisVpnPlugin.ERR_STARTUP_FAILED)
            invoke?.reject(error, "ANDROID_CORE_STOP_FAILED")
            return
        }
        running = false
        systemStarted = false
        request = null
        // 服务在起核途中被停掉：起核那条 Invoke 也要有回执，否则 Rust 侧只能等超时。
        val startInvoke = pendingStart
        pendingStart = null
        starting = false
        startInvoke?.reject("Android 隧道在起核途中被停止", PolarisVpnPlugin.ERR_STARTUP_FAILED)
        invoke?.resolve()
    }
}
