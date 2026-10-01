// 结构参考 sing-box-for-android 的 app/src/main/java/io/nekohasekai/sfa/Application.kt
// （Libbox.setup 的三个路径取值、单例句柄的组织方式）。
// Copyright (C) 2022 by nekohasekai <contact-sagernet@sekai.icu>
// 该项目按 GNU General Public License v3（或更新版本）分发，本文件因此继承 GPLv3。

package com.polaris2.app.vpn

import android.app.Application
import android.app.NotificationManager
import android.content.Context
import android.net.ConnectivityManager
import android.net.wifi.WifiManager
import android.util.Log
import com.polaris2.app.BuildConfig
import com.polaris2.app.PolarisTls
import io.nekohasekai.libbox.Libbox
import io.nekohasekai.libbox.SetupOptions

/**
 * 进程级单例：libbox 的一次性初始化 + 几个系统服务句柄。
 *
 * 为什么需要自定义 Application：`PlatformInterfaceWrapper` 的多数方法（枚举网卡、查连接归属、读
 * WiFi 状态）要 `Context`，但它们的调用者是 Go 侧，没有任何地方能把 Context 传进去。进程级单例
 * 是唯一的接法，上游同构。
 */
class PolarisApplication : Application() {
    override fun attachBaseContext(base: Context?) {
        super.attachBaseContext(base)
        instance = this
    }

    override fun onCreate() {
        super.onCreate()
        // One cold-process proof gates every libbox owner, including always-on starts.
        // This stat is outside the admission monitor; only ENOENT opens the ledger.
        AndroidNativeAdmissionGate.bootstrap(this)
        // The native tombstone is process-local; a new process may safely take
        // the persisted always-on endpoint as its first birth.
        NativeReconnectNotice.clear(this)
        DebugDiagnostics.install(this)
        // Assets live inside the APK, not next to /system/bin/app_process64.
        // Finish the small local copy before Rust's startup seeding can run.
        BundledRules.prepare(this)
        // 🔴 Rust 侧 TLS 栈的初始化，**必须同步、必须在这里**（见 PolarisTls 的类文档）：
        // Application.onCreate 是本进程最早的应用代码，早于任何 Activity，因而早于
        // Rust.create() → Tauri run() → AppRuntime::new()（reqwest client 在那里才建）
        // → 第一次 HTTPS。挪到后台线程或更晚的位置 = 把「初始化早于使用」从生命周期保证
        // 降级成赛跑。Rust 侧 run() 里有一道断言会当场揭穿这种降级。
        PolarisTls.initPlatformVerifier(this)
        // 放后台线程：setup 会触发 System.loadLibrary("box")，把一个 80 MB 级的 .so 映射进来。
        // 放主线程会把冷启动拖长且没有收益 —— 真正等它的是 VpnService，那条路径自己会 ensureSetup()。
        Thread({ ensureSetup() }, "polaris-libbox-setup").start()
    }

    companion object {
        private const val TAG = "PolarisLibbox"

        private lateinit var instance: PolarisApplication

        val application: PolarisApplication get() = instance

        val connectivity: ConnectivityManager by lazy {
            instance.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        }

        val wifi: WifiManager by lazy {
            instance.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        }

        val notification: NotificationManager by lazy {
            instance.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        }

        @Volatile
        private var setupError: Throwable? = null

        @Volatile
        private var setupDone = false

        /**
         * 幂等且阻塞：调用返回后 libbox 要么可用，要么把首次失败的原因抛出来。
         *
         * 失败不在这里吞掉：`Libbox.setup` 失败意味着 `.so` 没装进包、ABI 不匹配、或工作目录建不出来，
         * 之后每一次内核调用都会以更难懂的方式失败。让第一处失败带着原因冒泡，比留一串
         * UnsatisfiedLinkError 好诊断。
         */
        @Synchronized
        fun ensureSetup() {
            if (setupDone) return
            setupError?.let { throw it }
            try {
                val basePath = instance.filesDir.also { it.mkdirs() }
                val tempPath = instance.cacheDir.also { it.mkdirs() }
                // 外部私有目录可能为 null（存储被卸载），退回内部目录：内核只要求这三个路径可写。
                val workingPath = (instance.getExternalFilesDir(null) ?: basePath).also { it.mkdirs() }
                Libbox.setup(
                    SetupOptions().apply {
                        this.basePath = basePath.path
                        this.workingPath = workingPath.path
                        this.tempPath = tempPath.path
                        logMaxLines = 3000
                        debug = BuildConfig.DEBUG
                        appVersion = BuildConfig.VERSION_CODE.toString()
                        appMarketingVersion = BuildConfig.VERSION_NAME
                    },
                )
                setupDone = true
                // 这一行是「.so 真的被加载、JNI 桥真的通」的运行期收据：version() 是 native 方法，
                // 打得出来就意味着 dlopen + JNI 注册 + Go 运行时初始化三件都成了。
                Log.i(TAG, "libbox ready: version=${Libbox.version()} go=${Libbox.goVersion()} base=${basePath.path}")
            } catch (e: Throwable) {
                setupError = e
                Log.e(TAG, "libbox setup failed", e)
                throw e
            }
        }
    }
}
