//! Android 平台证书校验器（`rustls-platform-verifier`）的 JNI 初始化，以及「它到场了」的运行期断言。
//!
//! # 缺陷本体（2026-09-05 模拟器实测，非推断）
//!
//! ```text
//! I RustStdoutStderr: thread 'tokio-rt-worker' (10236) panicked at
//!   .../rustls-platform-verifier-0.7.0/src/android.rs:90:10:
//! I RustStdoutStderr: Expect rustls-platform-verifier to be initialized
//! ```
//!
//! ## 它是谁的依赖、怎么被拉进来的
//!
//! `src-tauri` → `reqwest 0.13.4`（features `rustls-no-provider` + `socks`）。reqwest 的
//! `rustls-no-provider` feature **无条件**含 `dep:rustls-platform-verifier`
//! （`reqwest-0.13.4/Cargo.toml:174-176`），Cargo.lock 里它也**只有** reqwest 一个消费者。
//! 客户端构建时（`reqwest-0.13.4/src/async_impl/client.rs:756-758`）：证书校验没关、主机名校验没关、
//! 没走 `tls_certs_only`、且 `root_certs` 为空 ⇒ 落进
//! `rustls_platform_verifier::Verifier::new(provider)` 这一支，并经
//! `with_custom_certificate_verifier`（同文件 783 行）成为该 `ClientConfig` 里**唯一**的
//! 服务端证书校验器。
//!
//! 本仓全部 reqwest 构造点都落在这一支：`runtime/http.rs` 的 `HttpRuntime::new` /
//! `with_local_proxy_url` / `build_warp_client`（另有一处 `#[cfg(test)]` 的
//! `with_resolve_overrides`）与 `runtime/http/subscription_transport.rs` 的
//! `build_direct_pinned_client` —— 全仓 `add_root_certificate` / `tls_certs_only` /
//! `danger_accept_invalid_*` 零命中。
//!
//! **不在射程内的那条**：解锁探测走 `crates/unlock-transport` 的 `wreq`（TLS 后端是 `btls`
//! = BoringSSL，不经 rustls），故它既不受本缺陷影响，也不受本次修复影响。
//!
//! ## 「没初始化」的后果是**连接失败**，不是回落到更弱的校验
//!
//! `Verifier::new` 自己不碰 JNI（`verification/android.rs:70-76` 只填两个字段），panic 发生在
//! 握手时的 `verify_server_cert` → `with_context()` → `global()`（`android.rs:87-90` 的 `expect`）。
//! rustls 的 `ClientConfig` 只持有**一个** `Arc<dyn ServerCertVerifier>`，没有第二条校验路径、
//! 也没有「校验器不可用就放行」的分支 —— 校验器 panic ⇒ 这次握手永远拿不到
//! `ServerCertVerified` ⇒ 连接就地作废。故它是 **fail-closed**：那次 HTTPS 失败，
//! 不存在「证书没验就连上了」的形态。
//!
//! panic 落在 `tokio-rt-worker` 上，被 tokio 的 task harness 接住（进程存活、UI 照常），
//! 于是表现为「某个后台功能静默不工作」——今天真机上第一条撞上它的是**规则资源库目录刷新**
//! （`runtime/rule_resource_scheduler.rs` 的 T+12s 启动补更 → `https://api.github.com/...`）。
//!
//! # 修法：进程最早期用 JNI 把 VM / Context / ClassLoader 交给它
//!
//! `rustls_platform_verifier::android::init_with_env(env, context)` 要一个 JNI `Env` 与一个
//! Android `Context`（它自己再 `getClassLoader()`）。本进程里**唯一**同时拿得到这两样、
//! 且早于一切 Rust 代码的位置是 `Application.onCreate`，故接缝是：
//!
//! ```text
//! PolarisApplication.onCreate()            (Kotlin, 进程内最早的应用代码)
//!   └─ PolarisTls.initPlatformVerifier(this)   external fun → JNI
//!        └─ Java_com_polaris2_app_PolarisTls_initPlatformVerifier   (本文件)
//!             └─ rustls_platform_verifier::android::init_with_env
//! ```
//!
//! 时序不是靠约定维持的，是 Android 生命周期给的：`Application.onCreate` 早于任何 Activity，
//! 因而早于 `WryLifecycleObserver.onCreate` → `Rust.create()` → `#[tauri::mobile_entry_point] run()`
//! → `AppRuntime::new()`（reqwest client 在这里才建）→ 第一次 HTTPS（最早 T+2s）。
//!
//! ## 为什么不改成 webpki-roots（换掉平台校验器）
//!
//! 换掉要在 `ClientBuilder` 上加根证书或开 `tls_certs_only`，那是 `runtime/http.rs` 的事；
//! 且代价是 Android 从此不认系统信任库的增删（企业 CA / 用户 CA / OS 侧的吊销与除名），
//! 与桌面三平台（那边 reqwest 同样走平台校验器）的信任锚**不再是同一套**。
//!
//! ## Kotlin 侧还需要一个 AAR
//!
//! 校验动作本体在 `org.rustls.platformverifier.CertificateVerifier`（Kotlin），由
//! `rustls-platform-verifier-android` crate 以 `.aar` 形态随 crate 分发。**只做本文件这半边
//! 而不装 AAR，症状会从「panic」变成「每次握手都报证书错」——一样不能用，还更难查**。
//! 装配在 `gen/android/app/build.gradle.kts`（`rustlsPlatformVerifierAar()`），
//! R8 的 keep 规则在 `gen/android/app/proguard-rules.pro`。
//!
//! # 判据
//!
//! - **运行期**（有牙的那条）：[`assert_ready_before_any_https`]，由 `lib.rs::run()` 在
//!   建 Tauri builder 之前无条件调用。接缝任一环断掉（Kotlin 调用被删 / 类名或方法名漂移 /
//!   `System.loadLibrary` 没执行）⇒ 启动即 panic 并点名，而不是等到 T+12s 某条后台腿静默失败。
//! - **源码级**：`src-tauri/tests/android_platform_verifier_wiring.rs` 对拍 Rust `#[jni_mangle]`
//!   参数、Kotlin `external fun` 声明、`PolarisApplication.onCreate` 的调用点、AAR 装配与 keep 规则。

use std::sync::atomic::{AtomicBool, Ordering};

use jni::objects::{JClass, JObject};
use jni::EnvUnowned;

/// Kotlin 侧持有 `external fun` 的类的全限定名。**改它必须同时改三处**：本常量、下方
/// `#[jni_mangle]` 的第一个实参、`PolarisTls.kt` 的 package + object 名。
/// 三处一致由 `tests/android_platform_verifier_wiring.rs` 对拍（漂移 ⇒ 门红）。
pub(crate) const JNI_CLASS: &str = "com.polaris2.app.PolarisTls";

/// Kotlin 侧那个 `external fun` 的方法名（同上，三处一致由门对拍）。
pub(crate) const JNI_METHOD: &str = "initPlatformVerifier";

/// 「`init_with_env` 已经成功返回」的进程级事实。
///
/// **不是** `rustls-platform-verifier` 的内部状态的镜像 —— 那个 crate 没有暴露 `is_initialized()`，
/// 而唯一能观测它的方式（发一次 HTTPS）恰好就是我们要保护的那条路径。故这里记的是本仓自己那次
/// 调用的结果：`init_with_env` 用 `OnceCell::get_or_try_init`，返回 `Ok` 即全局态已就位。
static READY: AtomicBool = AtomicBool::new(false);

/// `PolarisTls.initPlatformVerifier(context)` 的 Rust 实现。
///
/// 由 `PolarisApplication.onCreate` 在进程最早期调用一次（幂等：底层是 `OnceCell`）。
///
/// 失败（拿不到 ClassLoader / 建不了 global ref）→ 经 `ThrowRuntimeExAndDefault` 抛成 Java
/// `RuntimeException`：`Application.onCreate` 里抛异常 = 应用当场起不来并带着原因进 logcat。
/// 这比「悄悄没初始化、十二秒后某条后台腿 panic 在 tokio worker 上」好诊断整整一个量级 ——
/// 后者正是本模块要根治的失败形态本身。
#[jni::jni_mangle("com.polaris2.app.PolarisTls", "initPlatformVerifier")]
pub fn init_platform_verifier<'local>(
    mut unowned: EnvUnowned<'local>,
    _class: JClass<'local>,
    context: JObject<'local>,
) {
    unowned
        .with_env(|env| -> Result<(), jni::errors::Error> {
            rustls_platform_verifier::android::init_with_env(env, context)?;
            READY.store(true, Ordering::SeqCst);
            Ok(())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

/// 本进程是否已经把平台校验器初始化好了。
#[must_use]
pub(crate) fn is_ready() -> bool {
    READY.load(Ordering::SeqCst)
}

/// 🔴 **正面断言**：在本进程发出任何一次 HTTPS 之前，平台校验器必须已经到场。
///
/// 调用点在 `lib.rs::run()` 的最前面（早于 `tauri::Builder` / `AppRuntime::new` / 一切 spawn），
/// 故它断言的不是「将来某时会初始化」而是「**现在**已经初始化了」—— 这正是接线判据要的那半：
/// 光有 [`init_platform_verifier`] 这个函数存在，证明不了生产代码真的走到过它。
///
/// # 为什么是 panic 而不是记一条 error
///
/// 没到场时，本进程的每一次 HTTPS 都会在 rustls 的证书校验里 panic 掉一个 tokio worker：
/// 订阅更新 / 规则资源 / 内核与规则下载 / 解锁探测 / DoH sidecar / WARP 全部静默失效，
/// 而应用界面完好无损。「起得来但整个网络栈是死的」是比「起不来」隐蔽得多、也危险得多的形态 ——
/// 用一条 `log::error!` 换它，等于把本次修复的失败模式原样留在原地（而且此刻日志 sink 还没装，
/// 那条 error 只会进 stderr）。故这里选当场炸，且消息里写清补法。
///
/// # Panics
///
/// 接缝断掉（Kotlin 侧调用被删 / 改名 / `libpolaris_lib.so` 没加载）时 panic。
pub(crate) fn assert_ready_before_any_https() {
    assert!(
        is_ready(),
        "rustls-platform-verifier 未初始化：{JNI_CLASS}.{JNI_METHOD}() 没有在 \
         PolarisApplication.onCreate 里跑过。此刻起本进程的每一次 HTTPS 都会在证书校验处 \
         panic 掉一个 tokio worker（订阅/规则资源/更新/解锁/DoH 全失效）。\
         接线三处：src-tauri/src/android_tls.rs 的 #[jni_mangle]、\
         gen/android/app/src/main/java/com/polaris2/app/PolarisTls.kt、\
         gen/android/app/src/main/java/com/polaris2/app/vpn/PolarisApplication.kt 的 onCreate"
    );
}
