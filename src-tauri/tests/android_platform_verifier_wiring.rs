//! Android 平台证书校验器（`rustls-platform-verifier`）的**接线**门 —— 跨 Rust / Kotlin / Gradle 三侧。
//!
//! # 这条门守的是什么
//!
//! `src/android_tls.rs` 把「初始化」抽成了一个函数，而这一整类缺陷的形状恰恰是
//! **机制有门、接线没门**：函数写得完美、单测全绿，而生产代码根本没调它（或调在错的时刻、
//! 或调的是一个已经改了名的符号）。本门与 `android_tls::assert_ready_before_any_https`
//! 分工如下，两条缺一不可：
//!
//! | | 它证明的事 | 它证明不了的事 |
//! |---|---|---|
//! | 运行期断言（`lib.rs::run()` 里那行） | 真机上初始化**确实发生过**，且早于第一次 HTTPS | 它自己在不在、跑不跑得到（真机不跑就没人问它） |
//! | 本门（源码级） | 三侧的符号名逐字对得上、调用点在 `Application.onCreate` 的**同步直属语句**位置、AAR 与 keep 规则在位 | 运行期真的执行了 |
//!
//! # 为什么必须是源码级而不是单测
//!
//! 接缝的三个端点分别是 Rust 的 `#[jni_mangle]` 实参、Kotlin 的 `external fun` 声明、
//! 与 Kotlin 的调用语句。JNI 按**字符串**在运行期解析符号：任意一端改名，Rust 编译器与
//! Kotlin 编译器都毫无察觉，只有装到机器上才炸。除了逐字对拍源码，没有别的地方能表达它。
//!
//! # 取材与自污染
//!
//! 本门住在 `src-tauri/tests/`（不在任何 `src/` 取材面内），且只读**别的**文件；
//! 断言用的类名 / 方法名一律从 Rust 侧 `#[jni_mangle]` 的实参**提取**，不在本文件里另写一份
//! 字面量 —— 三处一起改名时本门照常绿，只有**不一致**才红。
//!
//! Rust 侧取材走 [`polaris_source_probe::mask_comments`]（保留字符串字面量：`#[jni_mangle("…")]`
//! 的实参本身就是字面量）与 [`polaris_source_probe::mask_comments_and_strings`]（符号面：
//! 断言调用了哪个函数）。Kotlin / Gradle / ProGuard 侧按行剥 `//` 与 `#` 注释后再取材，
//! 理由同上 —— 不剥的话，注释里写着的同一句话会给正面断言喂一份与生产代码无关的证据。

use std::collections::BTreeMap;

/// Rust 侧接缝实现文件（相对 `src-tauri/src/`）。
const RUST_MODULE: &str = "android_tls.rs";
/// Kotlin 侧 `external fun` 声明文件（相对 workspace 根）。
const KOTLIN_DECL: &str = "src-tauri/gen/android/app/src/main/java/com/polaris2/app/PolarisTls.kt";
/// Kotlin 侧调用点文件（相对 workspace 根）。
const KOTLIN_CALL: &str =
    "src-tauri/gen/android/app/src/main/java/com/polaris2/app/vpn/PolarisApplication.kt";
/// Kotlin 源码树（调用点唯一性的扫描面，相对 workspace 根）。
const KOTLIN_TREE: &str = "src-tauri/gen/android/app/src/main/java";
/// AAR 装配文件（相对 workspace 根）。
const GRADLE_APP: &str = "src-tauri/gen/android/app/build.gradle.kts";
/// R8 keep 规则文件（相对 workspace 根）。
const PROGUARD: &str = "src-tauri/gen/android/app/proguard-rules.pro";
/// Android 清单（相对 workspace 根）—— 决定 `Application` 子类会不会被系统加载。
const MANIFEST: &str = "src-tauri/gen/android/app/src/main/AndroidManifest.xml";

/// Rust 侧按名字 `loadClass` 的那个 Kotlin 类的**全限定名**。
///
/// # 为什么是一条登记，而不是从某个仓内文件提取
///
/// 这个名字的真值住在两处，都不在本仓：
///   · `rustls-platform-verifier-0.7.0/src/verification/android.rs:20` ——
///     `CachedClass::new(jni_str!("org.rustls.platformverifier.CertificateVerifier"))`；
///   · 随 `rustls-platform-verifier-android` crate 分发的那份 `.aar` —— `classes.jar` 里
///     恰好四个类，`org/rustls/platformverifier/CertificateVerifier.class` 是其一。
/// 两处都是 cargo registry 里的第三方内容：Android 侧依赖不进 Linux 主机的编译取材面，
/// 裸 checkout 上不保证在场，去提取它只会把本门变成一条会随机缺席的假门。
/// 故登记在这里并写清取证来源（2026-09-05 本机实测）。
///
/// # 它守的那条缺陷
///
/// 上一版两支门都只按**前缀**断言（`-keep class org.rustls.platformverifier.`）。
/// 实测：把规则改成 `-keep class org.rustls.platformverifier.NoSuchThing { *; }`，
/// 两支门全绿 —— 而 `CertificateVerifier` 此时一条 keep 都没有，R8 整包剪掉它，
/// release 每一次 TLS 握手都报证书错。前缀命中证明的是「有一条规则提到了这个包」，
/// 不是「那个类被保住了」。下面改为按 ProGuard 的通配语义判**覆盖**。
const RUSTLS_VERIFIER_CLASS: &str = "org.rustls.platformverifier.CertificateVerifier";

/// 剥掉行注释（`//` / `#`），保留其余全部字节与行结构。
///
/// **保长度不是可有可无**：失败信息里要报「第几行」，而且下面的花括号配平依赖偏移守恒。
/// 只剥行注释、不剥块注释：Kotlin 的 KDoc（`/** … */`）里确实写着本门要找的那些串，
/// 故块注释也要剥 —— 见 [`strip_comments`] 的实现。
fn strip_line_comment(line: &str, marker: &str) -> String {
    match line.find(marker) {
        Some(i) => {
            let mut out = String::with_capacity(line.len());
            out.push_str(&line[..i]);
            out.extend(std::iter::repeat_n(' ', line.len() - i));
            out
        }
        None => line.to_string(),
    }
}

/// 剥掉 Kotlin / Gradle 的 `//` 行注释与 `/* … */` 块注释（含 KDoc），保长度、保换行。
///
/// 直接借 [`polaris_source_probe::mask_comments`]：它逐个**跳过字符串 / 字符字面量**，
/// 于是字面量内部的 `//` 与 `/*` 不会被当成注释起笔。Kotlin 与 Rust 在这三种词法上同形
/// （`"…"` 带 `\` 转义、`'x'`、`/* */` 可嵌套），故可直接复用。
///
/// # 为什么不是手写的那版（2026-09-05 实测的一次假红）
///
/// 上一版是「见 `/` 加 `*` 就当块注释起笔」的手写扫描。`build.gradle.kts` 的 debug 块里有
/// `jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")` 这样的字符串 —— 里面同时带着 `/*` 与 `*/`。
/// 那四行恰好首尾相接地互相闭合，最后一处 `*/x86_64/*.so` 打开的「块注释」则一路吃到
/// release 块里 `include("**/*.pro")` 的 `*/` 才收口。也就是说：**那一整段被吃掉的源码
/// 从来就不在取材面上**，只是被吃掉的范围里恰好没有本门的针，所以没人发现。
///
/// 2026-09-05 把那个 `**/*.pro` 通配换成显式清单（`app/build/` 不该进 R8 配置）之后，
/// 收口的 `*/` 没了 —— 块注释一路吃到文件尾，`the_kotlin_verifier_aar_and_keep_rules_are_wired`
/// 当场红在「`rustlsPlatformVerifierAar()` 定义了却没有被 dependencies 消费」。
/// 那条红是**假的**：`dependencies` 里那行一直都在，只是净化面把它吃了。
/// 换成 `mask_comments` 后同一条断言恢复绿，而取材面从此不依赖「字符串里恰好有个 `*/`」。
fn strip_comments(src: &str) -> String {
    polaris_source_probe::mask_comments(src)
}

/// 剥掉 ProGuard 规则文件的 `#` 行注释。
fn strip_hash_comments(src: &str) -> String {
    src.lines()
        .map(|l| strip_line_comment(l, "#"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 从 `#[jni::jni_mangle("<class>", "<method>")]` 提取两个实参。
///
/// 提取失败 = 接缝的**源头**不在了（属性被删 / 改成不带实参的形态），直接红：
/// 下游全部断言都以它为准，源头缺席时若「静默跳过」，整条门会变成恒真。
fn jni_mangle_args(rust_src: &str) -> (String, String) {
    let attr = "#[jni::jni_mangle(";
    let start = rust_src.find(attr).unwrap_or_else(|| {
        panic!(
            "{RUST_MODULE} 里找不到 `{attr}…)` —— JNI 符号名的**唯一**真值源不见了。\
             Kotlin 侧的 `external fun` 将永远解析不到实现。"
        )
    });
    let rest = &rust_src[start + attr.len()..];
    let end = rest.find(')').unwrap_or_else(|| {
        panic!("{RUST_MODULE} 的 `{attr}` 没有闭合的 `)` —— 取材切片失败，不猜。")
    });
    let args: Vec<String> = rest[..end]
        .split(',')
        .map(|a| a.trim().trim_matches('"').to_string())
        .filter(|a| !a.is_empty())
        .collect();
    assert_eq!(
        args.len(),
        2,
        "{RUST_MODULE} 的 `{attr}…)` 期望 2 个字符串实参（类全名, 方法名），实得 {:?}。\
         只给类名时 jni-rs 会把 Rust 函数名转成 lowerCamelCase 当方法名 —— 那条隐式规则\
         没法在源码级对拍，故本仓要求显式写死两个。",
        args
    );
    (args[0].clone(), args[1].clone())
}

/// 提取某个 Kotlin/Gradle 源里第一处 `prefix` 之后、到 `stop` 为止的标识符。
fn token_after(src: &str, prefix: &str, stop: &[char], origin: &str) -> String {
    let at = src.find(prefix).unwrap_or_else(|| {
        panic!("{origin} 里找不到 `{prefix}` —— 取材切片失败，下游断言全部作废。")
    });
    let rest = &src[at + prefix.len()..];
    let end = rest.find(stop).unwrap_or(rest.len());
    let tok = rest[..end].trim().to_string();
    assert!(
        !tok.is_empty(),
        "{origin} 的 `{prefix}` 后面是空的 —— 切片自检失败。"
    );
    tok
}

/// 取一个 `{` 起手的块（含配平的花括号），返回块体（不含最外层花括号）。
fn balanced_block(src: &str, open_at: usize, origin: &str) -> String {
    let bytes = src.as_bytes();
    assert_eq!(
        bytes[open_at], b'{',
        "{origin}：块提取的起点不是 `{{` —— 切片自检失败。"
    );
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(open_at) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return src[open_at + 1..i].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("{origin}：从 {open_at} 起的花括号没有配平 —— 切片自检失败。")
}

/// ① Rust 侧：`#[jni_mangle]` 的实参、模块常量、真正调用的初始化 API，三者必须一致。
#[test]
fn rust_side_declares_the_jni_seam_and_calls_the_real_initializer() {
    let raw = polaris_source_probe::expect_marker(
        polaris_source_probe::crate_source!(RUST_MODULE),
        RUST_MODULE,
        "pub(crate) fn assert_ready_before_any_https",
    );
    // 字面量面（保留字符串）：`#[jni_mangle("…")]` 的实参与两个常量都是字面量。
    let literals = polaris_source_probe::mask_comments(&raw);
    // 符号面（剥掉字符串）：断言调用的是真 API，而不是注释/字符串里提到它。
    let symbols = polaris_source_probe::mask_comments_and_strings(&raw);

    let (class, method) = jni_mangle_args(&literals);
    assert!(
        class.contains('.') && !class.ends_with('.'),
        "{RUST_MODULE}：`#[jni_mangle]` 第一个实参应是类的全限定名（含包名），实得 `{class}`"
    );

    // 常量与属性实参同源：panic 文案里报的类名/方法名不许与真实符号漂移。
    for (konst, value) in [("JNI_CLASS", &class), ("JNI_METHOD", &method)] {
        let needle = format!("{konst}: &str = \"{value}\"");
        assert!(
            literals.contains(&needle),
            "{RUST_MODULE}：常量 `{konst}` 与 `#[jni_mangle]` 实参不一致 —— \
             期望源码里出现 `{needle}`。接线断言的失败文案会指错人。"
        );
    }

    // 正面断言：真的调了那个 crate 的初始化 API（不是自己写了个同名壳）。
    assert!(
        symbols.contains("rustls_platform_verifier :: android :: init_with_env")
            || symbols.contains("rustls_platform_verifier::android::init_with_env"),
        "{RUST_MODULE}：没有调用 `rustls_platform_verifier::android::init_with_env`。\
         没有它，`#[jni_mangle]` 导出的只是一个什么都不做的空壳 —— 三侧符号全对得上，\
         真机上照样在第一次握手时 panic。"
    );
    // 正面断言：成功路径上必须置位，否则运行期断言恒红/恒绿都可能。
    assert!(
        symbols.contains("READY.store(true"),
        "{RUST_MODULE}：初始化成功后没有把 READY 置位 —— \
         `assert_ready_before_any_https` 会在**已经初始化好**的进程上恒红。"
    );
}

/// ② Kotlin 声明侧：package + object + external fun 必须与 `#[jni_mangle]` 逐字拼得上。
#[test]
fn kotlin_external_declaration_matches_the_rust_jni_symbol() {
    let rust =
        polaris_source_probe::mask_comments(&polaris_source_probe::crate_source!(RUST_MODULE));
    let (class, method) = jni_mangle_args(&rust);

    let raw = polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(KOTLIN_DECL),
        KOTLIN_DECL,
        "System.loadLibrary",
    );
    let src = strip_comments(&raw);

    let package = token_after(&src, "package ", &['\n', '\r'], KOTLIN_DECL);
    let object = token_after(&src, "object ", &[' ', '{', '\n'], KOTLIN_DECL);
    let declared = format!("{package}.{object}");
    assert_eq!(
        declared, class,
        "JNI 类名两侧不一致：Rust `#[jni_mangle]` 说 `{class}`，\
         {KOTLIN_DECL} 是 `{declared}`。JNI 按字符串解析符号 —— 两侧编译器都不会报错，\
         装到机器上是 UnsatisfiedLinkError。"
    );

    let external = format!("external fun {method}(");
    assert!(
        src.contains(&external),
        "{KOTLIN_DECL} 里找不到 `{external}` —— 方法名与 Rust `#[jni_mangle]` 的第二个实参不一致。"
    );
    // `@JvmStatic`：object 里的 external fun 若不加它，JNI 第二参是 `this`（JObject）而非 JClass，
    // 且符号名不变 —— 参数错位是运行期才炸的那种错。
    let jvmstatic_at = src.find("@JvmStatic").unwrap_or_else(|| {
        panic!("{KOTLIN_DECL}：`{external}` 上没有 `@JvmStatic` —— JNI 实参形态会与 Rust 侧错位。")
    });
    let external_at = src.find(&external).expect("上面刚断言过它存在");
    assert!(
        jvmstatic_at < external_at,
        "{KOTLIN_DECL}：`@JvmStatic` 不在 `{external}` 之前 —— 它没有作用在这个声明上。"
    );
    assert!(
        src.contains("System.loadLibrary(\"polaris_lib\")"),
        "{KOTLIN_DECL} 没有加载 `polaris_lib`：本对象要在 `Rust` 被触及之前（Application.onCreate，\
         早于 Activity）就调进 native，那时 `Rust` 的 static 初始化还没跑过。"
    );
}

/// ③ 调用点：只能在 `PolarisApplication.onCreate` 里，且必须是它的**同步直属语句**。
///
/// 「唯一性」由整棵 Kotlin 树的扫描面给，不由夹具清单给：调用点被搬去 `MainActivity`
/// （太晚：Rust 的 `run()` 可能已经跑了）或包进 `Thread { … }`（变成赛跑）都要红。
#[test]
fn the_only_call_site_is_a_direct_statement_of_application_on_create() {
    let rust =
        polaris_source_probe::mask_comments(&polaris_source_probe::crate_source!(RUST_MODULE));
    let (class, method) = jni_mangle_args(&rust);
    let simple_class = class.rsplit('.').next().expect("类全名非空").to_string();
    let call = format!("{simple_class}.{method}(");

    // 扫描面 = 整棵 Kotlin 源码树（新增文件自动进面）。
    let files =
        polaris_source_probe::repo_dir_files_in(env!("CARGO_MANIFEST_DIR"), KOTLIN_TREE, "kt");
    let mut hits: BTreeMap<String, usize> = BTreeMap::new();
    for (path, body) in &files {
        let n = strip_comments(body).matches(&call).count();
        if n > 0 {
            hits.insert(path.clone(), n);
        }
    }
    assert_eq!(
        hits.keys().cloned().collect::<Vec<_>>(),
        vec![KOTLIN_CALL.to_string()],
        "`{call}` 的调用点必须**有且只有**一处，且在 {KOTLIN_CALL}。实测命中：{hits:?}\n\
         搬到 Activity = 晚于 Application.onCreate，可能已经错过 Rust 侧的第一次 HTTPS；\
         多处调用 = 有人在别的时刻又来一次，掩盖真正的时序问题。"
    );

    let raw = polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(KOTLIN_CALL),
        KOTLIN_CALL,
        "class PolarisApplication : Application()",
    );
    let src = strip_comments(&raw);

    // onCreate 的块体（配平花括号），而不是「文件里有这句」——后者对「写在别的方法里」是瞎的。
    let sig = "override fun onCreate() {";
    let sig_at = src.find(sig).unwrap_or_else(|| {
        panic!("{KOTLIN_CALL} 里找不到 `{sig}` —— Application 的 onCreate 没了或改了形态。")
    });
    let body = balanced_block(&src, sig_at + sig.len() - 1, KOTLIN_CALL);

    let call_at = body.find(&call).unwrap_or_else(|| {
        panic!(
            "{KOTLIN_CALL} 的 `onCreate` 块里没有 `{call}`。\n\
             这正是本轮要修的缺陷：初始化被抽成了函数，而生产代码没有在需要它的时刻调它 —— \
             真机上表现为第一次 HTTPS 在 rustls 证书校验处 panic 掉一个 tokio worker。"
        )
    });
    // 直属语句：调用点之前不许有任何未闭合的 `{`（否则它落在 lambda / Thread / if 里 = 不同步或有条件）。
    let before = &body[..call_at];
    let depth = before.matches('{').count() as i64 - before.matches('}').count() as i64;
    assert_eq!(
        depth, 0,
        "{KOTLIN_CALL}：`{call}` 不是 `onCreate` 的直属语句（它前面有 {depth} 层未闭合的 `{{`）。\
         包进 `Thread {{ … }}` / `if` / lambda 就把「初始化早于使用」从生命周期保证降级成赛跑。"
    );
    // 且必须在 super.onCreate() 之后（Application 的 Context 在那之前未必就绪）。
    let super_at = before.find("super.onCreate()").unwrap_or_else(|| {
        panic!("{KOTLIN_CALL}：`{call}` 出现在 `super.onCreate()` 之前 —— Context 未必已就绪。")
    });
    assert!(super_at < call_at, "断言已由 `before` 的取材面保证");
}

/// ④ 生产调用点（Rust 侧）：`run()` 必须在建 Tauri builder **之前**无条件断言校验器已到场。
#[test]
fn run_asserts_the_verifier_before_building_anything() {
    let raw = polaris_source_probe::expect_marker(
        polaris_source_probe::crate_source!("lib.rs"),
        "lib.rs",
        "pub fn run() {",
    );
    let src = polaris_source_probe::mask_comments_and_strings(&raw);

    let sig = "pub fn run() {";
    let run_at = src.find(sig).expect("哨兵已保证它存在");
    let body = balanced_block(&src, run_at + sig.len() - 1, "lib.rs");

    let guard_at = body
        .find("android_tls :: assert_ready_before_any_https")
        .or_else(|| body.find("android_tls::assert_ready_before_any_https"))
        .unwrap_or_else(|| {
            panic!(
                "lib.rs 的 `run()` 里没有调用 `android_tls::assert_ready_before_any_https()`。\n\
                 少了它，Kotlin 侧的初始化调用被删/改名时**没有任何东西会说话** —— \
                 应用照常起来、界面完好，只有后台每一条 HTTPS 静默失败。"
            )
        });
    let builder_at = body
        .find("tauri :: Builder :: default")
        .or_else(|| body.find("tauri::Builder::default"))
        .expect("run() 里必然有 tauri::Builder::default()");
    assert!(
        guard_at < builder_at,
        "lib.rs：断言必须在 `tauri::Builder::default()` 之前（实测 {guard_at} vs {builder_at}）。\
         排在后面 = 排在 `AppRuntime::new()`（reqwest client 在那里建）与全部 spawn 之后，\
         那时第一次 HTTPS 可能已经在飞。"
    );
    // `?` / 提前 return 都可能把它跳过：断言必须是 run() 的直属语句。
    let before = &body[..guard_at];
    let depth = before.matches('{').count() as i64 - before.matches('}').count() as i64;
    assert_eq!(
        depth, 0,
        "lib.rs：`assert_ready_before_any_https()` 不是 `run()` 的直属语句（前面有 {depth} 层未闭合 `{{`）。"
    );
}

/// ⑤ Kotlin 校验器本体（AAR）与 R8 keep 规则：少任何一条都是「不 panic 了，但每次握手报证书错」。
#[test]
fn the_kotlin_verifier_aar_and_keep_rules_are_wired() {
    let rust =
        polaris_source_probe::mask_comments(&polaris_source_probe::crate_source!(RUST_MODULE));
    let (class, _) = jni_mangle_args(&rust);

    let gradle = strip_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(GRADLE_APP),
        GRADLE_APP,
        "implementation(files(\"libs/libbox.aar\"))",
    ));
    assert!(
        gradle.contains("fun rustlsPlatformVerifierAar()"),
        "{GRADLE_APP}：没有 `rustlsPlatformVerifierAar()` —— \
         `org.rustls.platformverifier.CertificateVerifier` 不在包里，\
         Rust 侧 `load_class` 会抛 ClassNotFoundException，每次 TLS 握手都返证书错。"
    );
    assert!(
        gradle.contains("implementation(files(rustlsPlatformVerifierAar()))"),
        "{GRADLE_APP}：`rustlsPlatformVerifierAar()` 定义了却没有被 `dependencies` 消费 —— \
         这正是「机制有门、接线没门」的同一个形状。"
    );
    assert!(
        gradle.contains("rustls-platform-verifier-android"),
        "{GRADLE_APP}：AAR 的定位没有走 `cargo metadata` 里那个包名 —— \
         写死路径会在 crate 升版后悄悄用回旧 aar。"
    );

    let keeps = strip_hash_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(PROGUARD),
        PROGUARD,
        "proguardFiles",
    ));

    // ── 覆盖判定：至少一条**真能保住类本体**的 keep 按 ProGuard 通配语义罩得住那个类全名 ──
    let patterns = keep_class_patterns(&keeps);
    assert!(
        !patterns.is_empty(),
        "{PROGUARD}：一条 `-keep class …` 都没抠出来 —— 取材面塌了，下面的覆盖判定会恒假/恒真。"
    );
    // 切片自检的正面对照：本文件必然含 PolarisTls 那条，抠不出来就是提取器坏了而不是规则没了。
    assert!(
        patterns.iter().any(|rule| rule.pattern.contains("PolarisTls")),
        "{PROGUARD}：抠出了 {} 条 keep 模式却没有 PolarisTls 那条 —— 提取器与规则文件的形态对不上。",
        patterns.len()
    );
    // 🔴 先按**指令语义**筛一道（2026-09-05 收官轮 A8）：`-keepnames` 与 `-keepclassmembers`
    //    都不阻止 R8 把类剪掉，用它们写的规则在这里不算「罩住了」。
    let covering: Vec<&KeepRule> = patterns
        .iter()
        .filter(|rule| keep_directive_preserves_class(&rule.directive))
        .filter(|rule| proguard_pattern_covers(&rule.pattern, RUSTLS_VERIFIER_CLASS))
        .collect();
    assert!(
        !covering.is_empty(),
        "{PROGUARD}：没有任何一条**保得住类本体**的 keep 规则罩得住 `{RUSTLS_VERIFIER_CLASS}`。\n\
         现有的 keep 规则（指令 + 模式）：{patterns:?}\n\
         release 开着 minify，而那个类只被 JNI 反射按名字触达 —— R8 看不见引用就整包剪掉，\
         症状是 release 包每次 TLS 握手都报证书错（debug 包不 minify，复现不出来）。\n\
         两种失效形态本门都判：\n\
         · 模式不覆盖 —— `-keep class org.rustls.platformverifier.NoSuchThing` 含同一个包名前缀，\
         而它一个类都保不住；\n\
         · 指令不保类 —— `-keepnames`（= `-keep,allowshrinking`）与 `-keepclassmembers` 都**允许剪**：\
         前者只承诺「留下来的话不改名」，后者只管成员而类照剪。两者写出来与 `-keep` 几乎一样。\n\
         「规则罩住了」之后还剩一问「R8 真的保住了吗」，那由 release 冒烟腿的 seeds.txt 判据答\
         （scripts/assert-r8-evidence.mjs）。"
    );
    let keep_seam = format!("-keep class {class} ");
    assert!(
        keeps.contains(&keep_seam),
        "{PROGUARD}：缺 `{keep_seam}{{ *; }}` —— native 方法名就是 JNI 符号名的一部分，\
         被 R8 重命名即找不到实现。"
    );
}

/// ⑥ `AndroidManifest.xml` 必须把调用点所在的那个 `Application` 子类注册成本应用的 Application。
///
/// # 为什么需要这一条：上面五条合起来仍留着链条最顶上那一格
///
/// 2026-09-05 主会话的独立变异实测：把 manifest 里 `<application>` 的 `android:name` 整行删掉
/// (XML 仍合法),`PolarisApplication.onCreate` 于是**一次都不会跑**、整条初始化链彻底断掉 ——
/// 而上面五条门**全绿**(`5 passed; 0 failed`)。它们逐条守的是「调用点写对了没有」，
/// 没有一条守「写着调用点的那个类，系统会不会加载它」。这正是本文件头注那张表要防的形状
/// 又高了一层：**机制有门、接线有门，而承载接线的那个宿主没门**。
///
/// 运行期那条断言(`lib.rs::run()`)确实会因此当场 panic,所以它不是静默失败;
/// 但那要等到有人真的装一次 Android 包才现形，而本门在提交前就能红。
///
/// # 判据形态：两侧都**提取**,不在本文件写死字面量
///
/// 期望值 = `KOTLIN_CALL` 那个文件自己的 `package` 行 + 它自己的 `class <Name>` 声明;
/// 实得值 = manifest `<application>` 标签上的 `android:name`,按 Gradle `namespace` 展开相对形式
/// (`.vpn.PolarisApplication` ⇒ `com.polaris2.app.vpn.PolarisApplication`)。
/// 于是「三处一起改名」照常绿，只有**不一致**或**缺席**才红 —— 与本文件其余五条同一口径。
#[test]
fn the_manifest_registers_the_application_subclass_that_holds_the_call_site() {
    // ── 期望侧：从调用点文件自身提取全限定名 ──
    let call_src = strip_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(KOTLIN_CALL),
        KOTLIN_CALL,
        "override fun onCreate()",
    ));
    let pkg = token_after(&call_src, "package ", &['\n', '\r', ';'], KOTLIN_CALL);
    let class_at = call_src.find("class ").unwrap_or_else(|| {
        panic!("{KOTLIN_CALL} 里找不到 `class ` —— 取材切片失败，下游断言全部作废。")
    });
    let class_name = token_after(
        &call_src[class_at..],
        "class ",
        &[' ', ':', '(', '<', '{', '\n', '\r'],
        KOTLIN_CALL,
    );
    assert!(
        call_src.contains(&format!("{class_name} : Application()"))
            || call_src.contains(&format!("{class_name}: Application()")),
        "{KOTLIN_CALL}:`{class_name}` 不是 `Application` 的子类 —— \
         那么 manifest 的 `android:name` 指向它就没有意义，取材面选错了类。"
    );
    let expected_fqn = format!("{pkg}.{class_name}");

    // ── 实得侧：manifest 的 `<application android:name="…">`,按 Gradle namespace 展开 ──
    let namespace = {
        let gradle = strip_comments(&polaris_source_probe::expect_marker(
            polaris_source_probe::repo_file!(GRADLE_APP),
            GRADLE_APP,
            "namespace =",
        ));
        token_after(&gradle, "namespace = \"", &['"'], GRADLE_APP)
    };
    let manifest = polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(MANIFEST),
        MANIFEST,
        "<application",
    );
    let app_at = manifest.find("<application").expect("哨兵已保证它存在");
    let app_end = manifest[app_at..].find('>').unwrap_or_else(|| {
        panic!("{MANIFEST}:`<application` 标签没有闭合的 `>` —— 取材切片失败，不猜。")
    });
    let app_tag = &manifest[app_at..app_at + app_end];
    assert!(
        app_tag.contains("android:name=\""),
        "{MANIFEST}:`<application>` 上没有 `android:name` —— \
         系统会用默认的 `android.app.Application`,`{expected_fqn}.onCreate()` \
         **一次都不会跑**,平台校验器永远不初始化。\n\
         实测过：删掉这一行，本文件其余五条门全部照常绿。\n\
         补法：在 `<application>` 上写 `android:name=\"{}\"`。",
        expected_fqn
            .strip_prefix(&namespace)
            .unwrap_or(&expected_fqn)
    );
    let declared = token_after(app_tag, "android:name=\"", &['"'], MANIFEST);
    let declared_fqn = if let Some(rel) = declared.strip_prefix('.') {
        format!("{namespace}.{rel}")
    } else {
        declared.clone()
    };
    assert_eq!(
        declared_fqn, expected_fqn,
        "{MANIFEST}:`<application android:name=\"{declared}\">` 展开成 `{declared_fqn}`,\
         而写着 JNI 初始化调用点的类是 `{expected_fqn}`({KOTLIN_CALL})。\
         两者不一致 = 系统加载的是**另一个** Application,那句 \
         `PolarisTls.initPlatformVerifier(this)` 永远不会执行。"
    );
}

/// 一条 `-keep…` 规则拆出来的两半：指令（含修饰符）与它点名的类名模式。
#[derive(Debug, PartialEq, Eq)]
struct KeepRule {
    /// 指令原文，含修饰符，如 `-keep`、`-keepnames`、`-keep,allowshrinking`。
    directive: String,
    /// `class` 后面那个 token，如 `org.rustls.platformverifier.**`。
    pattern: String,
}

/// 抠出规则文件里全部 `-keep… class <模式>`：指令与类名模式**分开**返回。
///
/// 带注解限定（`-keep @Foo class Bar`）时 `class` 仍在，取的还是它后面那个 token。
fn keep_class_patterns(rules: &str) -> Vec<KeepRule> {
    let mut out = Vec::new();
    for line in rules.lines() {
        let line = line.trim();
        if !line.starts_with("-keep") {
            continue;
        }
        let Some(at) = line.find(" class ") else {
            continue;
        };
        let directive = line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        let rest = line[at + " class ".len()..].trim_start();
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '{')
            .unwrap_or(rest.len());
        let token = rest[..end].trim();
        if !token.is_empty() {
            out.push(KeepRule {
                directive,
                pattern: token.to_string(),
            });
        }
    }
    out
}

/// 这条指令**真的能阻止 R8 把类整包剪掉**吗？
///
/// # 为什么不能一概接受（2026-09-05 收官轮 A8）
///
/// 上一版的覆盖判定对 `-keep` 前缀一视同仁，于是把 `-keepnames` 与 `-keepclassmembers`
/// 也算成「罩住了」。ProGuard / R8 的语义里这两种都**不阻止剪枝**：
///
/// | 指令 | 保类本体？ | 语义 |
/// |---|---|---|
/// | `-keep` | ✅ | 保住类与点名的成员，不剪不改名 |
/// | `-keepclasseswithmembers` | ✅ | 类里有那些成员时，保住类本体 |
/// | `-keepnames` | ❌ | 等价 `-keep,allowshrinking`：**允许剪**，只是留下时不改名 |
/// | `-keepclassmembers` | ❌ | 只管成员；类本身该剪照剪（类没了，成员也没了） |
/// | `-keepclassmembernames` / `-keepclasseswithmembernames` | ❌ | 同样 allowshrinking |
/// | 任意带 `allowshrinking` 修饰符 | ❌ | 修饰符本身就是「允许剪」 |
///
/// 而本门要判的恰恰是「那个只被 JNI 按名字反射触达的类会不会被整包剪掉」。
/// 用一条 ❌ 指令写的规则在 configuration.txt 里长得和 ✅ 的一模一样，运行期后果却是
/// 每次 TLS 握手都报证书错。
///
/// `-keepclasseswithmembers` 算 ✅ 是有条件的：它只在类里真有那些成员时才生效。
/// 本仓今天没有用它写的规则；真要用时下面的成员对拍不在本门射程内，
/// 由 release 冒烟腿的 seeds.txt 判据答「R8 到底保住了没有」。
fn keep_directive_preserves_class(directive: &str) -> bool {
    let mut parts = directive.split(',');
    let head = parts.next().unwrap_or_default();
    let modifiers: Vec<&str> = parts.collect();
    if modifiers.iter().any(|m| m.trim() == "allowshrinking") {
        return false;
    }
    matches!(head, "-keep" | "-keepclasseswithmembers")
}

/// ProGuard / R8 的类名通配语义：`**` 罩任意串（含 `.`），`*` 罩任意不含 `.` 的串，
/// `?` 罩一个不是 `.` 的字符，其余逐字。
///
/// 手写回溯而不是引正则依赖：本仓的简约阶梯要求先看能不能不引依赖，而这里要判的语义只有三条。
fn proguard_pattern_covers(pattern: &str, class: &str) -> bool {
    fn go(p: &[u8], c: &[u8]) -> bool {
        if p.is_empty() {
            return c.is_empty();
        }
        if p[0] == b'*' {
            if p.len() >= 2 && p[1] == b'*' {
                // `**`：吃任意长度（含 `.`）
                for skip in 0..=c.len() {
                    if go(&p[2..], &c[skip..]) {
                        return true;
                    }
                }
                return false;
            }
            // `*`：吃任意长度，但不跨过 `.`
            for skip in 0..=c.len() {
                if c[..skip].contains(&b'.') {
                    break;
                }
                if go(&p[1..], &c[skip..]) {
                    return true;
                }
            }
            return false;
        }
        if c.is_empty() {
            return false;
        }
        if p[0] == b'?' {
            return c[0] != b'.' && go(&p[1..], &c[1..]);
        }
        p[0] == c[0] && go(&p[1..], &c[1..])
    }
    go(pattern.as_bytes(), class.as_bytes())
}

#[test]
fn the_proguard_pattern_matcher_has_teeth() {
    // 正：本仓今天那条规则罩得住目标类。
    assert!(proguard_pattern_covers(
        "org.rustls.platformverifier.**",
        RUSTLS_VERIFIER_CLASS
    ));
    // 反：验收员用过的那条变异罩不住 —— 没有这条对照，上面的覆盖判定可能是恒真的。
    assert!(!proguard_pattern_covers(
        "org.rustls.platformverifier.NoSuchThing",
        RUSTLS_VERIFIER_CLASS
    ));
    // `*` 不跨 `.`：单星只能罩一段。
    assert!(!proguard_pattern_covers(
        "org.rustls.*",
        RUSTLS_VERIFIER_CLASS
    ));
    assert!(proguard_pattern_covers(
        "org.rustls.platformverifier.*",
        RUSTLS_VERIFIER_CLASS
    ));
    // 逐字全等也算罩住。
    assert!(proguard_pattern_covers(
        RUSTLS_VERIFIER_CLASS,
        RUSTLS_VERIFIER_CLASS
    ));
    // 提取器自检：三种 keep 前缀与注解限定都取得到 class 后面那个 token，且指令原样带回来。
    let patterns = keep_class_patterns(
        "-keep class a.B { *; }\n-keepnames class c.D\n-keep @E.F class g.H { *; }\n-dontwarn i.J\n",
    );
    assert_eq!(
        patterns
            .iter()
            .map(|rule| (rule.directive.as_str(), rule.pattern.as_str()))
            .collect::<Vec<_>>(),
        vec![("-keep", "a.B"), ("-keepnames", "c.D"), ("-keep", "g.H")]
    );
}

/// A8 的对照：指令语义判定必须真的把「不保类本体」的那几种筛掉。
///
/// 没有这条，`keep_directive_preserves_class` 写成恒真（那正是上一版的行为）也没人说话。
#[test]
fn only_the_directives_that_really_pin_the_class_count_as_coverage() {
    // ✅ 真的保住类本体
    assert!(keep_directive_preserves_class("-keep"));
    assert!(keep_directive_preserves_class("-keepclasseswithmembers"));
    // ❌ allowshrinking 一族：类照剪，只是留下时不改名
    assert!(!keep_directive_preserves_class("-keepnames"));
    assert!(!keep_directive_preserves_class("-keepclassmembernames"));
    assert!(!keep_directive_preserves_class(
        "-keepclasseswithmembernames"
    ));
    // ❌ 只管成员：类本身没保住，成员也就跟着没了
    assert!(!keep_directive_preserves_class("-keepclassmembers"));
    // ❌ 修饰符直接写出来的那一形
    assert!(!keep_directive_preserves_class("-keep,allowshrinking"));
    assert!(keep_directive_preserves_class("-keep,allowobfuscation"));

    // 端到端：同一个类名，用 ❌ 指令写出来时覆盖判定必须为空。
    let weak = keep_class_patterns(&format!(
        "-keepnames class {RUSTLS_VERIFIER_CLASS}\n-keepclassmembers class {RUSTLS_VERIFIER_CLASS} {{ *; }}\n"
    ));
    assert_eq!(weak.len(), 2, "两条都该被抠出来（提取器不按语义筛）");
    assert!(
        weak.iter()
            .filter(|rule| keep_directive_preserves_class(&rule.directive))
            .all(|rule| !proguard_pattern_covers(&rule.pattern, RUSTLS_VERIFIER_CLASS)),
        "用 -keepnames / -keepclassmembers 写的规则被算成了「罩住了」—— 那是上一版的洞"
    );
    // 正向对照：换成 `-keep` 之后必须算罩住（否则上一条会被一个恒假的判定满足）。
    let strong = keep_class_patterns(&format!("-keep class {RUSTLS_VERIFIER_CLASS} {{ *; }}\n"));
    assert!(
        strong
            .iter()
            .any(|rule| keep_directive_preserves_class(&rule.directive)
                && proguard_pattern_covers(&rule.pattern, RUSTLS_VERIFIER_CLASS)),
        "`-keep class <全名>` 竟然不算罩住 —— 判定恒假，上面那条否定断言没有信息量"
    );
}
