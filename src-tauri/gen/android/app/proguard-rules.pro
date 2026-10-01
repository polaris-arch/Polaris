# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# ── 出事之后还查得动：保留源文件名与行号 ───────────────────────────────────────
#
# 这两行原本是 AGP 模板里注释掉的占位（本仓从未打开过）。打开的理由不是洁癖：
# 本应用**第一次**在 R8 开着的情况下跑，而 R8 剪错任何一条反射面的症状都是 **release 独有**的
# 运行期异常（debug 不 minify，复现不出来）。没有 SourceFile + LineNumberTable，logcat 里只剩
# 混淆后的类名与「行号 0」，连「炸在哪一行」都读不出来 —— 唯一的取证入口就此关闭。
#
# 现有规则里**没有**这两条：AGP 默认档 `proguard-android-optimize.txt`
# （= gradle-8.11.0.jar 内 com/android/build/gradle/proguard-{header,common,optimizations}.txt，
# 已逐行读过）的 `-keepattributes` 清单只有 AnnotationDefault / EnclosingMethod / InnerClasses /
# RuntimeVisible* / Signature，不含 SourceFile 与 LineNumberTable；
# proguard-tauri.pro 与 generated/proguard-wry.pro 里一条 `-keepattributes` 都没有。
#
# `-renamesourcefileattribute` 把源文件名统一改成常量 `SourceFile`：既拿到行号，又不把
# 原始文件名铺进包里。反混淆靠 build/outputs/mapping/<variant>/mapping.txt，不靠这个名字。
-keepattributes SourceFile,LineNumberTable
-renamesourcefileattribute SourceFile

# ── rustls-platform-verifier 的两个反射/JNI 面（release 开着 minify，必须显式 keep）──
#
# ① `org.rustls.platformverifier.CertificateVerifier`（来自 AAR）：Rust 侧经
#    `ClassLoader.loadClass("org.rustls.platformverifier.CertificateVerifier")` 加载，再按
#    **名字+签名**调它的静态方法（`verifyCertificateChain` 等），返回值是
#    `VerificationResult` / `StatusCode`。R8 看不见任何 Java 侧引用 ⇒ 不 keep 就整包被剪掉，
#    症状是 release 包每次 TLS 握手都报证书错（debug 包不 minify，因此这条只在 release 上有牙）。
# ② `com.polaris2.app.PolarisTls`：native 方法的名字就是 JNI 符号名的一部分
#    （`Java_com_polaris2_app_PolarisTls_initPlatformVerifier`），被重命名即找不到实现。
#
# 两条都写成 `{ *; }`：这两个类的成员**全部**只经反射 / JNI 触达，按成员挑等于挑一份必然漂移的清单。
-keep class org.rustls.platformverifier.** { *; }
-keep class com.polaris2.app.PolarisTls { *; }

# ── wry 自己生成的 keep 清单漏掉的两个 RustWebView 方法 ───────────────────────
#
# `generated/proguard-wry.pro`（Tauri CLI 生成，不入库、不该手改）的 RustWebView keep 块只列了
# `loadUrlMainThread` / `loadHTMLMainThread` / `evalScript` 三个。但 wry 在 Rust 侧还按名字调另外
# 两个：`clearAllBrowsingData()`（wry-0.55.1 src/android/main_pipe.rs:443 的
# `call_method(webview, "clearAllBrowsingData", "()V", …)`）与
# `getCookies(String)`（同文件 :464）。这两个方法在 Kotlin 侧**零调用点**
# （RustWebView.kt:80 与 :92 定义，全仓再无引用），也**不是** android.webkit.WebView 的覆写
# ——WebView 上根本没有这两个方法，所以「覆写库方法不会被改名」那条语义豁免够不到它们。
#
# 查过而**都没有**这两条的规则来源：本仓 proguard-rules.pro（本文件其余部分）、
# proguard-tauri.pro（只有 TauriActivity.getPluginManager）、generated/proguard-wry.pro:27-33、
# AGP 默认三份（其中 `-keepclassmembers public class * extends android.view.View { void set*(***);
# *** get*(); }` 不匹配：`get*()` 限定**无参**，而 getCookies 带一个 String 形参）、
# libbox.aar 的 proguard.txt（go.** / io.nekohasekai.**）、androidx.webkit 1.14.0 的 proguard.txt
# （只管 org.chromium.support_lib_boundary 与 androidx.webkit）、tauri-android 的 consumer 规则
# （只管 app.tauri.**）。
#
# 今天打不出来：`clear_all_browsing_data` 与 cookie 相关的 Tauri 命令都挂 `#[cfg(desktop)]`
# （tauri-2.11.5 src/webview/plugin.rs），本仓 Rust/TS 侧对它们的调用点为 0。这是**潜伏缺口**：
# 哪天有人在 Rust 侧调一次 `webview.cookies_for_url(...)` 或 `clear_all_browsing_data()`，
# release 上就是 NoSuchMethodError 而 debug 全绿，且那时没有任何门会红。两行的代价是 dex 里
# 多留两个方法名。
-keep class com.polaris2.app.RustWebView {
  void clearAllBrowsingData();
  java.lang.String getCookies(java.lang.String);
}

# ── R8 的 "Missing classes" 硬失败：jackson 引用了两个 Android 上不存在的 JDK 类 ──────
#
# 不加这两行，配好凭据后**第一次** `./gradlew :app:assembleArm64Release` 就会在
# `:app:minifyArm64ReleaseWithR8` 上报 `ERROR: Missing classes detected while running R8`
# 并点名这两个类 —— 不是运行期风险，是构建当场失败。
#
# 取证（三条都是本机实测，不是推断）：
#   ① 谁引用：`com.fasterxml.jackson.core:jackson-databind:2.15.3` 里
#      `com/fasterxml/jackson/databind/ext/Java7SupportImpl.class` 的常量池含
#      `java/beans/ConstructorProperties` 与 `java/beans/Transient`（777 个 class 里只有这一个）。
#      它怎么进来的：`tauri-android`（mobile/android/build.gradle.kts）有
#      `implementation("com.fasterxml.jackson.core:jackson-databind:2.15.3")`，
#      本工程 `implementation(project(":tauri-android"))` ⇒ 进运行期类路径 ⇒ 进 R8 输入。
#      对拍 debug APK 的 dex：jackson 有 1038 个类在包里，`Java7SupportImpl` 是其中之一。
#   ② 为什么缺：`platforms/android-36/android.jar` 的 `java/beans/` 下**只有** 5 个
#      PropertyChange* 类，没有 ConstructorProperties、没有 Transient。
#   ③ 为什么没人替我们挡：jackson 三个 jar 一条 consumer proguard 规则都不带；
#      libbox.aar 的 proguard.txt 只有 go.** / io.nekohasekai.**；五个 tauri 项目的
#      consumer 规则里没有 dontwarn；AGP 默认三份档里没有 dontwarn。
#      过宽取材面复核：整个 gradle module 缓存 293 个 jar/aar 的 consumer 规则里，
#      提到 java.beans 的是 0 个（同一台扫描机器改问 sun.misc 命中 6 个 ⇒ 0 是真 0）。
#
# 为什么是 `-dontwarn` 而不是 `-keep`：`-keep` 表达的是「这个类在输入里，别剪它」——
# 而这两个类**根本不在任何输入里**，`-keep` 对不存在的类是空操作，R8 照样报 Missing classes。
# 要表达的语义是「这条引用永远走不到，别为它报错」，那正是 `-dontwarn`。
# 走不到的依据在 jackson 自己的代码：`Java7Support.instance()` 用 try/catch 包住
# `Java7SupportImpl` 的构造（构造器里那两条 `ldc` 就是探针），加载失败即回落到 null 实现。
#
# 为什么逐个点名而不是 `-dontwarn java.beans.**`：通配会把**将来**新出现的 java.beans 引用
# 一并静默吞掉。今天的取材面（上面②的 dex 全量对差）说清了恰好是这两个；第三个出现时
# 应该当场红一次让人重新判断，而不是被这行规则悄悄咽下去。
-dontwarn java.beans.ConstructorProperties
-dontwarn java.beans.Transient

# ── 第三条：只以**数组形态**出现的元素类型，上一轮的取材面看不见 ────────────────
#
# `com.google.errorprone:error_prone_annotations:2.15.0` 的两个注解
# `IncompatibleModifiers` / `RequiredModifiers` 各有一个 `Modifier[] value()`，于是
# `javax/lang/model/element/Modifier` 在 class 文件里**只**以 `()[Ljavax/lang/model/element/Modifier;`
# 这个方法描述符的形态出现 —— 它不是一条 `CONSTANT_Class` 项，只按类型项取材的扫描器一个都收不到。
# 而 `platforms/android-36/android.jar` 的 `javax/lang/` 下**一个条目都没有**。
#
# 取证（2026-09-05 本机实测，face 与来源都写清楚）：
#   ① 谁引用：只有这两个注解类，各 1 处，且都在方法描述符里（上面那个形状）。
#   ② 它在不在包里：`com.google.errorprone:error_prone_annotations:2.15.0` 出现在
#      `:app:dependencies --configuration arm64ReleaseRuntimeClasspath` 的解析结果里 ⇒ 进 R8 输入。
#   ③ 有没有人替我们挡：那个 jar 里一条 consumer proguard 规则都没有（`proguard`/`.pro` 条目为 0）。
#   ④ 正向对照（证明这次扫描确实找得到东西）：同一次扫描把已知的
#      `java.beans.{ConstructorProperties,Transient}` 与 coroutines 那 4 条原样复现了出来。
#
# **没有实测的一件事**：R8 到底会不会为它硬失败。判断它的唯一地方是一次真实的 R8 运行
# （android.yml 的 release 冒烟腿），本机没有那份产物。加这一行的依据是同形先例：
# kotlinx-coroutines 自己的 consumer 规则里就带着
# `-dontwarn org.codehaus.mojo.animal_sniffer.IgnoreJRERequirement`，注释写的正是
# 「An annotation used for build tooling, won't be directly accessed」—— 注解侧的缺失类
# R8 是会报的。代价是一行；反方向的代价是首次 release 构建当场失败。
-dontwarn javax.lang.model.element.Modifier

# ── 姊妹腿：上一轮那句「已扫过一遍，不用加」是**错的**，这里如实改写 ──────────────
#
# 上一轮记的是：把 debug APK 的 11 份 dex 拆开，取「引用到的类型」减「包内定义的类型」再减
# android.jar，剩 15 个 —— 9 个 `dalvik/annotation/*`、2 个 java.beans、4 个
# `java.lang.instrument.{Instrumentation,ClassFileTransformer}` 与 `sun.misc.{Signal,SignalHandler}`
# （引用方 `kotlinx.coroutines.debug.AgentPremain`，coroutines 自己的
# `META-INF/com.android.tools/proguard/coroutines.pro` 里带着那 4 条 `-dontwarn`）。
# 那 15 个的结论今天仍成立，**但「一共只有 15 个」这句全称否定不成立**：那次的取材面只收
# 类型项，漏掉了只在方法/字段描述符里以数组形态出现的元素类型（上面第三条就是这么漏掉的）。
#
# 2026-09-05 的补扫（换了一个面，故两次互为独立对照）：
#   face  = `arm64ReleaseRuntimeClasspath` 上解析出的 78 个坐标对应的 66 份 jar/aar
#           ＋ `app/libs/libbox.aar` ＋ rustls-platform-verifier 的 aar；
#   提取  = 每份 class 的常量池，取 `CONSTANT_Class`（数组描述符**拆开**）
#           ＋ 每一条描述符形状的 UTF8 项里的 `L…;`；
#   判定  = 引用集 −（这批 archive 自己定义的 8186 个类）−（android-36 的 6227 个类）。
# 结果（剔掉构建期才生成的 `R` / `R$*` 之后）里，除本文件已处理的之外只剩四类，都不用加：
#   · `org.codehaus.mojo.animal_sniffer.IgnoreJRERequirement` —— coroutines 的 consumer 规则里
#     已有同名 `-dontwarn`（上面引的就是那一行）；
#   · `java.lang.invoke.{LambdaMetafactory,StringConcatFactory}` —— D8/R8 的 desugar 契约面，
#     不是缺失类；
#   · `com.android.tools.lint.**` / `com.intellij.**` —— 来自 aar 内的 `lint.jar`，
#     那份不进包也不进 R8（本次扫描面**比 R8 的输入宽**，这几条是这个宽出来的部分）；
#   · 四个像 `oadTask` / `inkedIterator` 的碎片 —— 正则在字符串常量上误命中，不是类型。
# 这个面**窄于** R8 的真实输入的地方也要写明：应用自身与 `tauri-android` 现场编出来的 class
# 不在里面（本机没有那两侧的编译产物）。它们的缺失引用只有一次真实 R8 运行才说得清。
