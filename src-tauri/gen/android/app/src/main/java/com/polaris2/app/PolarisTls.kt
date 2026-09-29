package com.polaris2.app

import android.content.Context

/**
 * Rust 侧 TLS 栈的 Android 接缝：把 JNI 环境与应用 `Context` 交给
 * `rustls-platform-verifier`。
 *
 * # 为什么必须有这一步（缺陷本体，2026-09-05 模拟器实测）
 *
 * Rust 侧的 HTTP 客户端是 reqwest + rustls，其服务端证书校验器在 Android 上是
 * `rustls_platform_verifier::Verifier`（reqwest 在没有自定义根证书时**只**选它）。
 * 那个校验器要调 Android 的 TrustManager，因而需要 JVM 与一个 `Context`；拿不到就在
 * 第一次 TLS 握手时 panic：
 *
 * ```text
 * thread 'tokio-rt-worker' panicked at rustls-platform-verifier-0.7.0/src/android.rs:90:10:
 * Expect rustls-platform-verifier to be initialized
 * ```
 *
 * panic 落在 tokio worker 上被 task harness 接住 ⇒ 界面完好、**那次 HTTPS 失败**
 * （rustls 没有第二条校验路径，故是 fail-closed，不存在「没验证书就连上」的形态）。
 * 症状表现为订阅更新 / 规则资源 / 内核与规则下载 / 解锁探测 / DoH 静默不工作。
 *
 * # 调用点为什么在 `Application.onCreate`
 *
 * 它是本进程里**最早**的应用代码：早于任何 Activity，因而早于
 * `WryLifecycleObserver.onCreate` → `Rust.create()` → Tauri 的 `run()` → `AppRuntime::new()`
 * （Rust 的 reqwest client 在那里才建）→ 第一次 HTTPS（最早 T+2s）。
 * 「初始化早于使用」因此由 Android 生命周期保证，不靠本仓的排序自觉。
 *
 * Rust 侧另有一道运行期断言（`android_tls::assert_ready_before_any_https`，在 `run()` 最前面）：
 * 本调用一旦被删或改名，应用启动即 panic 并点名，而不是十二秒后某条后台腿悄悄失败。
 *
 * # 另外半边：Kotlin 校验器实现来自 AAR
 *
 * 真正做校验的 `org.rustls.platformverifier.CertificateVerifier` 由
 * `rustls-platform-verifier-android` crate 以 `.aar` 分发，装配见
 * `gen/android/app/build.gradle.kts` 的 `rustlsPlatformVerifierAar()`。
 * 少了它，症状会从 panic 变成「每次握手都报证书错」—— 一样不能用，还更难查。
 */
object PolarisTls {
    init {
        // 与 `Rust.kt` 同名同库：`System.loadLibrary` 幂等，同一 classloader 内第二次是 no-op。
        // 这里显式加载，是因为本对象要在 `Rust` 被触及**之前**（Application.onCreate，
        // 早于 Activity）就调进 native。
        System.loadLibrary("polaris_lib")
    }

    /**
     * 实现在 `src-tauri/src/android_tls.rs`（`#[jni_mangle("com.polaris2.app.PolarisTls",
     * "initPlatformVerifier")]`）。改包名 / 对象名 / 方法名要三处同改，
     * 由 `src-tauri/tests/android_platform_verifier_wiring.rs` 对拍。
     *
     * 幂等（Rust 侧是 `OnceCell`）。初始化失败会抛 `RuntimeException`：
     * 这一步失败意味着整个网络栈不可用，让它当场带着原因崩，比静默起来好诊断。
     */
    @JvmStatic
    external fun initPlatformVerifier(context: Context)
}
